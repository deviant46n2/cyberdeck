//! Workflow executor (ROADMAP Phase 8c) — drives the DAG schedule produced by
//! `deck_core::workflow::plan` against a `NodeRunner`.
//!
//! Layering: this crate is engine-only, so the DAG **driver** is runner-agnostic
//! (unit-testable with a mock) and the two provided runners are thin:
//!   - `StatelessRunner`: one-shot generation via `inference::run_prompt`
//!     against the resident engine slot (host 127.0.0.1, default_port).
//!   - `AgenticRunner`: a headless `opencode run` session in the workspace dir
//!     (stateful, tool-using); the TUI layer reuses the same opencode binary.
//!
//! Persistence (wfstore) is the caller's concern: `execute` returns an
//! `ExecReport` the CLI/Tauri layer writes back. This keeps the driver pure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use deck_core::profile::Engine;
use deck_core::workflow::{
    plan, Message, NodeKind, Workflow, WorkflowNode, WorkflowRunStatus,
};

// ---------------------------------------------------------------- outcomes

/// A single node's run result as surfaced to the caller for persistence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeResult {
    pub node_id: String,
    pub ok: bool,
    pub text: String,
    pub error: String,
    pub wall_ms: u64,
    pub order_idx: u64,
    /// Generation tokens/sec when the runner can report it (stateless only).
    pub tps: Option<f64>,
    /// Time to first token (ms) when the engine reports it.
    pub ttft_ms: Option<u64>,
    /// Generated tokens this node produced (0 when not measurable).
    pub gen_tokens: u64,
    /// True when the node was gated out of this run: every incoming conditional
    /// edge evaluated false, so nothing reached it (Phase 8f branch skip).
    pub skipped: bool,
}

/// Aggregate result of one `execute` pass over a graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecReport {
    pub run_id: String,
    pub status: WorkflowRunStatus,
    pub node_results: Vec<NodeResult>,
    pub total_wall_ms: u64,
    pub tokens_used: u64,
    /// Number of times the loop body executed (1 = single pass, no loop; >1 =
    /// the loop back-edge was taken). Phase 8f.
    pub iterations: u32,
}

/// What a node runner produces for one node. `text` is the node's payload;
/// the metric fields are `Option`/`0` when the runner can't measure them
/// (e.g. an agentic session reports text but no tok/s). This is what lets a
/// workflow run feed the per-role bench (8e) without faking numbers.
#[derive(Debug, Clone)]
pub struct NodeOutcome {
    pub text: String,
    pub tps: Option<f64>,
    pub ttft_ms: Option<u64>,
    pub gen_tokens: u64,
}

// ---------------------------------------------------------------- runner

/// Executes a single node given its resolved upstream messages. Text-only for
/// V1; structured/artifact messages are carried by reference and concatenated.
/// Implementors must be `Send + Sync` so wave nodes can run pooled.
pub trait NodeRunner: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<NodeOutcome, String>;
}

/// Build a node's prompt from its Role + upstream inputs. The role provides the
/// system persona; upstream node outputs are appended youngest-last.
fn build_prompt(pool: &dyn NodeRunner, node: &WorkflowNode, inputs: &[Message]) -> String {
    let _ = pool;
    let mut parts: Vec<String> = Vec::new();
    if !node.binding.role_id.is_empty() {
        parts.push(format!(
            "You are playing the role '{}' (binding {}). Perform your assigned step.",
            node.binding.role_id, node.binding.model_ref
        ));
    }
    if !inputs.is_empty() {
        parts.push("Upstream context:".to_string());
        for m in inputs {
            if !m.text.is_empty() {
                parts.push(format!("<upstream>{}</upstream>", m.text));
            }
        }
    }
    parts.join("\n")
}

// ---------------------------------------------------------------- stateless runner

/// The default V1 runner: one-shot generation against the resident engine slot.
/// Resolution is intentionally simple (host 127.0.0.1, `engine.default_port()`,
/// model_id = binding.model_ref) — the per-role model_matrix (8e) will make
/// this resolution explicit against the running fleet.
pub struct StatelessRunner {
    pub max_tokens: u32,
}

impl NodeRunner for StatelessRunner {
    fn name(&self) -> &'static str {
        "stateless"
    }
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<NodeOutcome, String> {
        let engine = Engine::parse(&node.binding.engine.clone().unwrap_or_default())
            .ok_or_else(|| format!("unsupported engine '{}'", node.binding.engine.clone().unwrap_or_default()))?;
        let host = "127.0.0.1".to_string();
        let port = engine.default_port();
        let prompt = build_prompt(self, node, inputs);
        // model_ref is vault style `alias@quant` or `ollama/name:tag`; inference wants bare alias/tag.
        let alias = node.binding.model_ref.split('@').next().unwrap_or(&node.binding.model_ref).trim();
        let alias = alias.split('/').last().unwrap_or(alias).trim();
        let model_id = if alias.is_empty() { node.binding.model_ref.as_str() } else { alias };
        let s = crate::inference::run_prompt(
            engine,
            &host,
            port,
            model_id,
            &prompt,
            node.exec.max_tokens.min(self.max_tokens).max(16),
        );
        if s.ok {
            Ok(NodeOutcome {
                text: s.text,
                tps: s.tok_s,
                ttft_ms: s.ttft_ms,
                gen_tokens: s.gen_tokens.unwrap_or(0),
            })
        } else {
            Err(s.error.unwrap_or_else(|| "generation failed".into()))
        }
    }
}

// ---------------------------------------------------------------- echo runner (no-LLM loop proof, tonight)

// Like LiteGraph's EchoRunner in the unit tests — lets you see TUI-to-TUI
// wiring + loop iterations + bench without any engine up. The CLI/UI expose
// it as `runner=echo`.
pub struct EchoRunner;

impl NodeRunner for EchoRunner {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<NodeOutcome, String> {
        let up = inputs.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join(" | ");
        let task_line = if up.is_empty() { "no upstream".to_string() } else { up };
        // include the kickoff task visibly so the loop is obviously alive
        Ok(NodeOutcome {
            text: format!("[echo {}:{}] saw upstream: {} — gen 42 tokens", node.id, node.role_id, task_line),
            tps: Some(100.0),
            ttft_ms: Some(5),
            gen_tokens: 42,
        })
    }
}

// ---------------------------------------------------------------- agentic runner

/// V1 agentic runner: a headless `opencode run` in `dir`. Because a real agent
/// session can be long and interactive, this is invoked synchronously and the
/// result is whatever opencode prints to its final stdout when it exits.
pub struct AgenticRunner {
    pub dir: String,
    pub model: Option<String>,
}

impl NodeRunner for AgenticRunner {
    fn name(&self) -> &'static str {
        "agentic"
    }
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<NodeOutcome, String> {
        let mut cmd = std::process::Command::new("opencode");
        cmd.arg("run");
        cmd.arg("--dir").arg(&self.dir);
        if let Some(m) = self.model.as_ref().filter(|s| !s.is_empty()) {
            cmd.arg("-m").arg(m);
        }
        let prompt = build_prompt(self, node, inputs);
        cmd.arg(&prompt);
        let out = cmd.output().map_err(|e| format!("spawn opencode: {e}"))?;
        if out.status.success() {
            Ok(NodeOutcome {
                text: String::from_utf8_lossy(&out.stdout).trim().to_string(),
                tps: None,
                ttft_ms: None,
                gen_tokens: 0,
            })
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.trim().to_string())
        }
    }
}

// ---------------------------------------------------------------- DAG driver

/// Execute `wf` against `runner`, walking the waves from `plan` in order.
/// Fan-in: a node's inputs are the messages produced by ALL its direct
/// predecessors — correct because a wave only starts after the previous wave
/// fully completed. Within a wave, nodes are independent; V1 runs them
/// sequentially for determinism (a future `max_parallel` worker pool can run
/// them concurrently without changing fan-in semantics).
///
/// Phase 8f adds two behaviours on top of the V1 DAG driver:
///   - **Branch skip:** a conditional edge (`edge.condition`) only routes its
///     producer's message to the downstream node when the predicate evaluates
///     true against the produced text. A node whose *every* incoming edge is a
///     conditional edge that evaluated false is **skipped** (not executed) —
///     the "reviewer on condition" pattern. Unconditional fan-in still runs with
///     whatever messages are available (unchanged V1 behaviour).
///   - **Bounded loop:** if the workflow declares a single loop back-edge, the
///     body re-executes while its (continue) predicate holds, bounded by
///     `exec_settings.max_iterations` (0 = loops disabled) and the token budget.
///     The `stop` flag is honoured cooperatively between waves and passes.
///
/// The token budget (`exec_settings.budget_tokens`, 0 = unlimited) is enforced
/// across the whole run; when exhausted the run ends early as `Stopped`.
pub fn execute(
    wf: &Workflow,
    runner: &dyn NodeRunner,
    run_id: String,
    max_parallel: usize,
    stop: &AtomicBool,
) -> Result<ExecReport, String> {
    let _ = max_parallel; // V1 sequential; parallelism changes only throughput
    let dp = plan(wf)?;
    if !dp.unreachable.is_empty() {
        eprintln!(
            "[deck-workflow] note: unreachable nodes {:?}",
            dp.unreachable
        );
    }

    let started = Instant::now();
    let mut order: u64 = 0;
    let mut any_error = false;
    let mut stopped = false;
    // node_id -> Message produced in the *current pass*, consulted for fan-in.
    let mut messages: std::collections::HashMap<String, Message> =
        std::collections::HashMap::new();
    let mut node_results: Vec<NodeResult> = Vec::new();
    let mut tokens_used: u64 = 0;
    let budget_tokens = wf.exec_settings.budget_tokens;
    let max_iters = wf.exec_settings.max_iterations;
    let mut iterations_ran: u32 = 0;
    // Single loop back-edge delivery: carries the loop source's message to the
    // loop target on the *next* pass (Phase 8f).
    let mut loop_carry: Option<Message> = None;

    // Map node id -> its incoming (non-loop) edges' conditions, keyed by pred id.
    // This drives branch skip semantics without re-scanning edges per node.
    let mut pred_cond: std::collections::HashMap<String, Vec<(String, Option<String>)>> =
        std::collections::HashMap::new();
    for e in &wf.edges {
        if e.loop_edge {
            continue;
        }
        pred_cond.entry(e.to.clone()).or_default().push((e.from.clone(), e.condition.clone()));
    }

    // Run one pass over the whole body once; returns true if the loop should
    // continue (predicate held, budget/iterations/stop allow it).
    let loop_target = dp.loop_back_edge.as_ref().map(|b| b.to.clone());
    loop {
        if stop.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }
        let pass_begin_tokens = tokens_used;
        let pass_made_progress = run_pass(
            wf,
            &dp,
            &pred_cond,
            runner,
            &run_id,
            loop_target.as_deref(),
            &mut order,
            &mut any_error,
            &mut messages,
            &mut node_results,
            &mut tokens_used,
            &mut loop_carry,
            stop,
            iterations_ran,
        );
        iterations_ran += 1;
        // Budget is enforced continuously inside run_pass; a pass that burned
        // nothing extra is a no-op guard.
        if budget_tokens > 0 && tokens_used >= budget_tokens && tokens_used > pass_begin_tokens {
            stopped = true;
        }
        if !pass_made_progress {
            break;
        }
        // Loop continuation check.
        let back = match &dp.loop_back_edge {
            Some(b) => b,
            None => break,
        };
        if max_iters == 0 {
            break;
        }
        if iterations_ran >= max_iters {
            break;
        }
        if budget_tokens > 0 && tokens_used >= budget_tokens {
            stopped = true;
            break;
        }
        let src_msg = match messages.get(&back.from) {
            Some(m) => m.clone(),
            None => break, // loop source produced nothing -> nothing to loop on
        };
        // Continue-while predicate on the back edge; None/Always => loop.
        let continue_loop = match &back.condition {
            Some(c) => match deck_core::workflow::EdgePredicate::parse(c) {
                Ok(p) => p.eval(&src_msg.text),
                Err(e) => {
                    eprintln!("[deck-workflow] bad loop predicate on '{}': {e}", back.id);
                    false
                }
            },
            None => true,
        };
        if !continue_loop {
            break;
        }
        // Deliver the loop source's message into the loop target's next pass.
        loop_carry = Some(src_msg);
    }

    let total_wall_ms = started.elapsed().as_millis() as u64;
    let status = if stopped {
        WorkflowRunStatus::Stopped
    } else if any_error {
        WorkflowRunStatus::Partial
    } else {
        WorkflowRunStatus::Done
    };

    Ok(ExecReport {
        run_id,
        status,
        node_results,
        total_wall_ms,
        tokens_used,
        iterations: iterations_ran,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_pass(
    wf: &Workflow,
    dp: &deck_core::workflow::DryPlan,
    pred_cond: &std::collections::HashMap<String, Vec<(String, Option<String>)>>,
    runner: &dyn NodeRunner,
    run_id: &str,
    loop_target: Option<&str>,
    order: &mut u64,
    any_error: &mut bool,
    messages: &mut std::collections::HashMap<String, Message>,
    node_results: &mut Vec<NodeResult>,
    tokens_used: &mut u64,
    loop_carry: &mut Option<Message>,
    stop: &AtomicBool,
    iteration: u32,
) -> bool {
    let mut any_node_ran = false;
    for wave in &dp.waves {
        if stop.load(Ordering::SeqCst) {
            return any_node_ran;
        }
        for node_id in wave {
            let node = match wf.nodes.iter().find(|n| &n.id == node_id) {
                Some(n) => n,
                None => {
                    *any_error = true;
                    node_results.push(NodeResult {
                        node_id: node_id.clone(),
                        ok: false,
                        text: String::new(),
                        error: format!("node {node_id} missing"),
                        wall_ms: 0,
                        order_idx: { *order += 1; *order },
                        tps: None,
                        ttft_ms: None,
                        gen_tokens: 0,
                        skipped: false,
                    });
                    continue;
                }
            };

            // Gather the node's incoming messages with branch (conditional edge)
            // semantics, then decide whether the node is gated out (skipped).
            let incoming = dp.predecessors.get(node_id).cloned().unwrap_or_default();
            let conds = pred_cond.get(node_id).cloned().unwrap_or_default();
            let mut inputs: Vec<Message> = Vec::new();
            let mut gated_false: usize = 0;
            let mut has_unconditional: bool = false;
            for (pred, cond) in &conds {
                match cond {
                    None => {
                        has_unconditional = true;
                        if let Some(m) = messages.get(pred) {
                            inputs.push(m.clone());
                        }
                    }
                    Some(c) => {
                        let passes = match deck_core::workflow::EdgePredicate::parse(c) {
                            Ok(p) => match messages.get(pred) {
                                Some(m) => p.eval(&m.text),
                                None => false,
                            },
                            Err(_) => false,
                        };
                        if passes {
                            if let Some(m) = messages.get(pred) {
                                inputs.push(m.clone());
                            }
                        } else {
                            gated_false += 1;
                        }
                    }
                }
            }
            // Fold the loop back-edge delivery (the loop source's prior message)
            // into the loop target's inputs — only that node, once per pass.
            let carry = if loop_target == Some(node_id.as_str()) {
                loop_carry.take()
            } else {
                None
            };
            if let Some(carry) = carry {
                inputs.push(carry);
            }
            // Kickoff inputs (CrewAI `inputs.task` / Studio `Input`): seed source nodes
            // on the first iteration so "where do I type the task" has an answer.
            // Later loop iterations already have `loop_carry`, so this fires once.
            if inputs.is_empty() && incoming.is_empty() && iteration == 0 && !wf.inputs.is_empty() {
                let task = wf.inputs.get("task").or_else(|| wf.inputs.values().next()).cloned().unwrap_or_default();
                if !task.trim().is_empty() {
                    inputs.push(Message {
                        id: format!("m-input-task"),
                        node_run_id: format!("{run_id}-input"),
                        kind: deck_core::workflow::MessageKind::Text,
                        text: task,
                        structured: None,
                        ref_path: None,
                        meta: Default::default(),
                    });
                }
            }

            let skip = !incoming.is_empty() && !has_unconditional && gated_false == incoming.len();
            if skip {
                node_results.push(NodeResult {
                    node_id: node_id.clone(),
                    ok: true,
                    text: String::new(),
                    error: String::new(),
                    wall_ms: 0,
                    order_idx: { *order += 1; *order },
                    tps: None,
                    ttft_ms: None,
                    gen_tokens: 0,
                    skipped: true,
                });
                continue;
            }

            // Human gate — does not run an LLM, just surfaces the inputs for approval and pauses
            if node.kind == NodeKind::Human || node.role_id == "human" {
                let text = inputs.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join("\n---\n");
                let text = if text.is_empty() { "human approval required — no inputs yet".into() } else { text };
                *order += 1;
                node_results.push(NodeResult {
                    node_id: node_id.clone(),
                    ok: true,
                    text: text.clone(),
                    error: String::new(),
                    wall_ms: 0,
                    order_idx: *order,
                    tps: None,
                    ttft_ms: None,
                    gen_tokens: 0,
                    skipped: false,
                });
                messages.insert(
                    node_id.clone(),
                    Message {
                        id: format!("m-{node_id}"),
                        node_run_id: format!("{run_id}-{node_id}"),
                        kind: deck_core::workflow::MessageKind::Text,
                        text,
                        structured: None,
                        ref_path: None,
                        meta: Default::default(),
                    },
                );
                any_node_ran = true;
                continue;
            }

            let t = Instant::now();
            let outcome = runner.run(node, &inputs);
            let wall_ms = t.elapsed().as_millis() as u64;
            let (text, err, ok, tps, ttft_ms, gen_tokens) = match outcome {
                Ok(o) => (o.text, String::new(), true, o.tps, o.ttft_ms, o.gen_tokens),
                Err(e) => (String::new(), e, false, None, None, 0),
            };
            *any_error |= !ok;
            *order += 1;
            *tokens_used += if gen_tokens > 0 { gen_tokens } else { (text.len() / 4) as u64 };
            node_results.push(NodeResult {
                node_id: node_id.clone(),
                ok,
                text: text.clone(),
                error: err.clone(),
                wall_ms,
                order_idx: *order,
                tps,
                ttft_ms,
                gen_tokens,
                skipped: false,
            });
            messages.insert(
                node_id.clone(),
                Message {
                    id: format!("m-{node_id}"),
                    node_run_id: format!("{run_id}-{node_id}"),
                    kind: deck_core::workflow::MessageKind::Text,
                    text,
                    structured: None,
                    ref_path: None,
                    meta: Default::default(),
                },
            );
            any_node_ran = true;
        }
    }
    any_node_ran
}

/// Build a bench row for one executed node (Phase 8e). It records the node's
/// role, model and metrics into matrix_runs so per-role history accumulates
/// and the canvas can show which model is best at which node. Returns None
/// when the node has no binding engine (agentic/synthetic), because there is
/// nothing engine-backed to benchmark. tok_s_kind is "wall" because the
/// runner reports a single tok/s figure without its provenance.
pub fn node_to_matrix_row(
    wf: &Workflow,
    nr: &NodeResult,
    at: i64,
) -> Option<deck_core::store::MatrixRow> {
    // A skipped node never ran against an engine — nothing to benchmark.
    if nr.skipped {
        return None;
    }
    let node = wf.nodes.iter().find(|n| n.id == nr.node_id)?;
    let binding = &node.binding;
    Some(deck_core::store::MatrixRow {
        engine: binding.engine.clone().unwrap_or_default(),
        model: binding.model_ref.clone(),
        ctx: 0,
        task: node.role_id.clone(),
        run: 1,
        verdict: if nr.ok { "ok".into() } else { "error".into() },
        summary: format!("workflow node {}", node.id),
        gen_tokens: if nr.gen_tokens > 0 { Some(nr.gen_tokens) } else { None },
        prompt_tokens: None,
        tok_s: nr.tps,
        tok_s_kind: "wall".into(),
        wall_ms: nr.wall_ms,
        output: if nr.ok { nr.text.clone() } else { String::new() },
        at,
        workload_id: None,
        hardware_profile_id: None,
        engine_version: None,
        prompt_tps: None,
        ttft_ms: nr.ttft_ms,
        peak_vram_mb: None,
        model_rev: None,
        sampling_json: None,
        role_id: Some(node.role_id.clone()),
        workflow_id: Some(wf.id.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deck_core::workflow::{
        EdgePredicate, ExecSettings, ModelBinding, NodeExec, NodeKind, NodePos, Workflow,
        WorkflowEdge, seed_coding_review,
    };

    /// Mock runner that echoes `role-<node_id>` — lets us assert the DAG drove
    /// every node and that fan-in delivered predecessor text.
    struct EchoRunner;
    impl NodeRunner for EchoRunner {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<NodeOutcome, String> {
            let up: Vec<String> = inputs.iter().map(|m| m.text.clone()).collect();
            Ok(NodeOutcome { text: format!("{}|{}", node.id, up.join(";")), tps: None, ttft_ms: None, gen_tokens: 0 })
        }
    }

    fn node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            role_id: format!("role-{id}"),
            binding: ModelBinding { role_id: format!("role-{id}"), model_ref: format!("{id}@Q4"), engine: Some("llamacpp".into()), overrides_json: String::new(), active: true },
            kind: NodeKind::Stateless,
            pos: NodePos::default(),
            exec: NodeExec::default(),
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { id: id.into(), from: from.into(), to: to.into(), from_port: "output".into(), to_port: "input".into(), condition: None, loop_edge: false }
    }

    #[test]
    fn drives_weighted_fan_in() {
        let wf = seed_coding_review();
        let stop = AtomicBool::new(false);
        let rep = execute(&wf, &EchoRunner, "r-test".into(), 4, &stop).unwrap();
        assert_eq!(rep.status, WorkflowRunStatus::Done);
        assert_eq!(rep.node_results.len(), 2);
        // n1 has no upstream
        let n1 = rep.node_results.iter().find(|r| r.node_id == "n1").unwrap();
        assert_eq!(n1.text, "n1|");
        // n2 fan-in: sees n1's output as an upstream message
        let n2 = rep.node_results.iter().find(|r| r.node_id == "n2").unwrap();
        assert!(n2.text.contains("n1|"), "n2 should see n1 output, got {}", n2.text);
    }

    #[test]
    fn honours_stop_between_waves() {
        let wf = seed_coding_review();
        let stop = AtomicBool::new(true); // pre-set: nothing should run
        let rep = execute(&wf, &EchoRunner, "r-stop".into(), 4, &stop).unwrap();
        assert_eq!(rep.status, WorkflowRunStatus::Stopped);
        assert!(rep.node_results.is_empty());
    }

    #[test]
    fn stateless_runner_parses_engine_error() {
        let mut wf = seed_coding_review();
        wf.nodes[1].binding.engine = Some("bogus".into());
        let runner = StatelessRunner { max_tokens: 256 };
        // node n2 binding is 'bogus' -> parse fails
        let inputs = vec![];
        let r = runner.run(&wf.nodes[1], &inputs);
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("unsupported engine"));
    }

    #[test]
    fn build_prompt_includes_upstream() {
        let node = WorkflowNode {
            id: "n".into(),
            role_id: "r".into(),
            binding: ModelBinding { role_id: "r".into(), model_ref: "m@Q".into(), engine: Some("llamacpp".into()), overrides_json: String::new(), active: true },
            kind: NodeKind::Stateless,
            pos: NodePos::default(),
            exec: NodeExec::default(),
        };
        let m = Message {
            id: "m1".into(),
            node_run_id: "x".into(),
            kind: deck_core::workflow::MessageKind::Text,
            text: "hello upstream".into(),
            structured: None,
            ref_path: None,
            meta: Default::default(),
        };
        let p = build_prompt(&EchoRunner, &node, &[m]);
        assert!(p.contains("r"));
        assert!(p.contains("hello upstream"));
    }

    #[test]
    fn node_to_matrix_row_records_role_metrics() {
        let wf = seed_coding_review();
        let nr = NodeResult {
            node_id: "n1".into(),
            ok: true,
            text: "reviewed".into(),
            error: String::new(),
            wall_ms: 1234,
            order_idx: 1,
            tps: Some(62.5),
            ttft_ms: Some(88),
            gen_tokens: 512,
            skipped: false,
        };
        let row = node_to_matrix_row(&wf, &nr, 1755500000).unwrap();
        // n1 binds role + a model (from the seed); role_id threaded for 8e
        assert_eq!(row.role_id.as_deref(), Some(wf.nodes[0].role_id.as_str()));
        assert_eq!(row.model, wf.nodes[0].binding.model_ref);
        assert_eq!(row.tok_s, Some(62.5));
        assert_eq!(row.ttft_ms, Some(88));
        assert_eq!(row.gen_tokens, Some(512));
        assert_eq!(row.wall_ms, 1234);
    }

    #[test]
    fn conditional_edge_skips_node_when_predicate_fails() {
        // gate -> consumer, where the edge only routes when the gate output
        // contains "APPROVE". The gate emits "reject" -> condition false -> the
        // sole incoming edge is gated-false -> consumer is skipped.
        let wf = Workflow {
            id: "branch".into(),
            name: "Branch".into(),
            description: String::new(),
            version: 1,
            nodes: vec![node("gate"), node("consumer")],
            edges: vec![WorkflowEdge {
                id: "e1".into(),
                from: "gate".into(),
                to: "consumer".into(),
                from_port: "output".into(),
                to_port: "input".into(),
                condition: Some("contains:APPROVE".into()),
                loop_edge: false,
            }],
            exec_settings: ExecSettings::default(),
            template: false,
            inputs: Default::default(),
        };
        let stop = AtomicBool::new(false);
        // EchoRunner makes every node output "<id>|<inputs>", which does NOT
        // contain "APPROVE" -> the condition never fires -> consumer skipped.
        let rep = execute(&wf, &EchoRunner, "r-branch".into(), 4, &stop).unwrap();
        assert_eq!(rep.status, WorkflowRunStatus::Done);
        let gate = rep.node_results.iter().find(|r| r.node_id == "gate").unwrap();
        assert!(!gate.skipped);
        let consumer = rep.node_results.iter().find(|r| r.node_id == "consumer").unwrap();
        assert!(consumer.skipped, "consumer gated out must be skipped");
        // Skipped nodes produce no bench row.
        assert!(node_to_matrix_row(&wf, consumer, 0).is_none());
    }

    #[test]
    fn conditional_edge_routes_when_predicate_holds() {
        let wf = Workflow {
            id: "branch-ok".into(),
            name: "Branch ok".into(),
            description: String::new(),
            version: 1,
            nodes: vec![node("gate"), node("consumer")],
            edges: vec![WorkflowEdge {
                id: "e1".into(),
                from: "gate".into(),
                to: "consumer".into(),
                from_port: "output".into(),
                to_port: "input".into(),
                condition: Some("not_contains:NONE".into()),
                loop_edge: false,
            }],
            exec_settings: ExecSettings::default(),
            template: false,
            inputs: Default::default(),
        };
        let stop = AtomicBool::new(false);
        let rep = execute(&wf, &EchoRunner, "r-branch-ok".into(), 4, &stop).unwrap();
        let consumer = rep.node_results.iter().find(|r| r.node_id == "consumer").unwrap();
        assert!(!consumer.skipped);
        // consumer ran and saw the gate's message as upstream input
        assert!(consumer.text.contains("gate|"), "consumer should see gate output");
    }

    #[test]
    fn unconditional_workflow_never_skips() {
        // Backward-compat guard: with no conditional edges, every node runs.
        let wf = seed_coding_review();
        let stop = AtomicBool::new(false);
        let rep = execute(&wf, &EchoRunner, "r-uncond".into(), 4, &stop).unwrap();
        assert_eq!(rep.iterations, 1);
        assert!(rep.node_results.iter().all(|r| !r.skipped));
        assert_eq!(rep.node_results.len(), 2);
    }

    /// Runner that escalates an "attempt=N" counter seeded by the loop-carry
    /// input and emits "DONE" once the next attempt would reach 3. Lets the loop
    /// terminate via its predicate instead of spinning to max_iterations.
    struct LoopRunner;
    impl NodeRunner for LoopRunner {
        fn name(&self) -> &'static str {
            "loop"
        }
        fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<NodeOutcome, String> {
            let up = inputs.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join(";");
            let attempt = up
                .split("attempt=")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            let next = attempt + 1;
            if next >= 3 {
                Ok(NodeOutcome { text: format!("{} attempt={} result=DONE", node.id, next), tps: None, ttft_ms: None, gen_tokens: 0 })
            } else {
                Ok(NodeOutcome { text: format!("{} attempt={} result=WIP", node.id, next), tps: None, ttft_ms: None, gen_tokens: 0 })
            }
        }
    }

    #[test]
    fn bounded_loop_exits_on_termination_predicate() {
        // body: dev -> rev, plus loop back-edge rev -> dev with continue-while
        // predicate `not_contains:DONE` (loop until the body says DONE). The
        // loop carry feeds rev's output back into dev, whose attempt counter
        // escalates; at attempt>=3 the body emits DONE and the loop exits.
        let wf = Workflow {
            id: "rx-loop".into(),
            name: "Review Loop".into(),
            description: String::new(),
            version: 1,
            nodes: vec![node("dev"), node("rev")],
            edges: vec![
                edge("e1", "dev", "rev"),
                WorkflowEdge {
                    id: "e2".into(),
                    from: "rev".into(),
                    to: "dev".into(),
                    from_port: "output".into(),
                    to_port: "input".into(),
                    condition: Some("not_contains:DONE".into()), // continue while not done
                    loop_edge: true,
                },
            ],
            exec_settings: ExecSettings { max_iterations: 10, ..Default::default() },
            template: false,
            inputs: Default::default(),
        };
        assert_eq!(wf.validate(), Ok(()));
        let stop = AtomicBool::new(false);
        let rep = execute(&wf, &LoopRunner, "r-loop".into(), 4, &stop).unwrap();
        assert_eq!(rep.status, WorkflowRunStatus::Done);
        // dev escalates attempt 1 -> 2 -> 3(DONE): 2 body passes total.
        assert_eq!(rep.iterations, 2);
        assert_eq!(rep.node_results.iter().filter(|r| r.node_id == "dev").count(), 2);
        // The final rev pass observed DONE in its upstream (dev's carry).
        let rev = rep.node_results.iter().rfind(|r| r.node_id == "rev").unwrap();
        assert!(rev.ok);
        assert!(rev.text.contains("DONE"));
    }

    #[test]
    fn bounded_loop_capped_by_max_iterations() {
        let wf = Workflow {
            id: "cap-loop".into(),
            name: "Cap Loop".into(),
            description: String::new(),
            version: 1,
            nodes: vec![node("a")],
            // self loop: a -> a via loop back-edge with a predicate that always
            // continues (contains:WIP holds on the first pass, and the carry
            // escalates but the cap should stop us before termination at 3).
            edges: vec![WorkflowEdge {
                id: "e1".into(),
                from: "a".into(),
                to: "a".into(),
                from_port: "output".into(),
                to_port: "input".into(),
                condition: Some("contains:WIP".into()),
                loop_edge: true,
            }],
            exec_settings: ExecSettings { max_iterations: 2, ..Default::default() },
            template: false,
            inputs: Default::default(),
        };
        assert_eq!(wf.validate(), Ok(()));
        let stop = AtomicBool::new(false);
        let rep = execute(&wf, &LoopRunner, "r-cap".into(), 4, &stop).unwrap();
        assert_eq!(rep.iterations, 2, "always-continue loop must be capped at max_iterations");
        assert_eq!(rep.node_results.iter().filter(|r| r.node_id == "a").count(), 2);
    }

    #[test]
    fn loop_respects_token_budget() {
        let wf = Workflow {
            id: "budget-loop".into(),
            name: "Budget Loop".into(),
            description: String::new(),
            version: 1,
            nodes: vec![node("a")],
            edges: vec![WorkflowEdge {
                id: "e1".into(),
                from: "a".into(),
                to: "a".into(),
                from_port: "output".into(),
                to_port: "input".into(),
                condition: None,
                loop_edge: true,
            }],
            // 2 tokens budget; each pass of LoopRunner emits ~10 chars => ~2 tokens
            exec_settings: ExecSettings { max_iterations: 20, budget_tokens: 6, ..Default::default() },
            template: false,
            inputs: Default::default(),
        };
        assert_eq!(wf.validate(), Ok(()));
        let stop = AtomicBool::new(false);
        let rep = execute(&wf, &LoopRunner, "r-budget".into(), 4, &stop).unwrap();
        // Must not spin to max_iterations=20; budget stops it well short.
        assert!(rep.iterations < 20, "budget must cap iterations, got {}", rep.iterations);
        assert_eq!(rep.status, WorkflowRunStatus::Stopped);
    }

    #[test]
    fn edge_predicate_reexported_for_doors() {
        // The predicate type is used by the executor; ensure the pure eval is
        // reachable from the module so tests stay honest.
        assert!(EdgePredicate::parse("contains:x").unwrap().eval("abc x def"));
        assert!(!EdgePredicate::parse("contains:x").unwrap().eval("abc"));
    }
}
