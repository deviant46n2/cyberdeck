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
    plan, Message, Workflow, WorkflowNode, WorkflowRunStatus,
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
}

/// Aggregate result of one `execute` pass over a graph.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecReport {
    pub run_id: String,
    pub status: WorkflowRunStatus,
    pub node_results: Vec<NodeResult>,
    pub total_wall_ms: u64,
    pub tokens_used: u64,
}

// ---------------------------------------------------------------- runner

/// Executes a single node given its resolved upstream messages. Text-only for
/// V1; structured/artifact messages are carried by reference and concatenated.
/// Implementors must be `Send + Sync` so wave nodes can run pooled.
pub trait NodeRunner: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<String, String>;
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
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<String, String> {
        let engine = Engine::parse(&node.binding.engine.clone().unwrap_or_default())
            .ok_or_else(|| format!("unsupported engine '{}'", node.binding.engine.clone().unwrap_or_default()))?;
        let host = "127.0.0.1".to_string();
        let port = engine.default_port();
        let prompt = build_prompt(self, node, inputs);
        let s = crate::inference::run_prompt(
            engine,
            &host,
            port,
            &node.binding.model_ref,
            &prompt,
            node.exec.max_tokens.min(self.max_tokens).max(16),
        );
        if s.ok {
            Ok(s.text)
        } else {
            Err(s.error.unwrap_or_else(|| "generation failed".into()))
        }
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
    fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<String, String> {
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
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
/// Honours the `stop` flag cooperatively (between waves) and enforces the token
/// budget; both are the caller's loop-safety handle.
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
    // node_id -> Message produced so far, consulted for fan-in.
    let mut messages: std::collections::HashMap<String, Message> =
        std::collections::HashMap::new();
    let mut node_results: Vec<NodeResult> = Vec::new();
    let mut tokens_used: u64 = 0;

    for wave in &dp.waves {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        for node_id in wave {
            let node = wf
                .nodes
                .iter()
                .find(|n| &n.id == node_id)
                .ok_or_else(|| format!("node {node_id} missing"))?;
            let inputs: Vec<Message> = dp
                .predecessors
                .get(node_id)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|p| messages.get(p).cloned())
                .collect();

            let t = Instant::now();
            let outcome = runner.run(node, &inputs);
            let wall_ms = t.elapsed().as_millis() as u64;
            let (text, err, ok) = match outcome {
                Ok(t) => (t, String::new(), true),
                Err(e) => (String::new(), e, false),
            };
            any_error |= !ok;
            order += 1;
            tokens_used += (text.len() / 4) as u64; // ~4 chars/token heuristic
            node_results.push(NodeResult {
                node_id: node_id.clone(),
                ok,
                text: text.clone(),
                error: err.clone(),
                wall_ms,
                order_idx: order,
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
        }
    }

    let total_wall_ms = started.elapsed().as_millis() as u64;
    let status = if stop.load(Ordering::SeqCst) {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deck_core::workflow::{ModelBinding, NodeExec, NodeKind, NodePos, seed_coding_review};

    /// Mock runner that echoes `role-<node_id>` — lets us assert the DAG drove
    /// every node and that fan-in delivered predecessor text.
    struct EchoRunner;
    impl NodeRunner for EchoRunner {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn run(&self, node: &WorkflowNode, inputs: &[Message]) -> Result<String, String> {
            let up: Vec<String> = inputs.iter().map(|m| m.text.clone()).collect();
            Ok(format!("{}|{}", node.id, up.join(";")))
        }
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
}
