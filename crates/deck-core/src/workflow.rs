//! Infinite Agent Canvas — workflow domain (ROADMAP Phase 8c).
//!
//! **Pure, I/O-free domain model + DAG scheduler.** Lives here in `deck-core`
//! so the graph logic is unit-testable headless and shared by every door
//! (CLI + Tauri + canvas UI). Execution (`deck-engines::workflow`) consumes the
//! schedule `plan` produces.
//!
//! Core decision (from CANVAS.md): a Node is a **Role** bound to a **Model**,
//! not "a model." Roles are stable identities (Architecture Reviewer); a
//! Binding is the swappable assignment (Role ⟵ Qwen3.8-27B-NVFP4). This is what
//! lets one workflow run across a model matrix and accumulate per-role
//! benchmark intelligence.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------- Roles

/// A stable, model-agnostic job description. Deliberately has NO model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Role {
    pub id: String, // slug: 'architecture-reviewer'
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub system_prompt: String,
    #[serde(default)]
    pub instructions: String,
    /// Expected input message shape (optional; free-form hint for now).
    #[serde(default)]
    pub input_contract: String,
    /// Produced output shape (optional).
    #[serde(default)]
    pub output_contract: String,
    /// Tool whitelist / permission ids fed to the opencode agent config.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Inference defaults: temperature, top_p, top_k, ctx, max_tokens, reasoning.
    #[serde(default)]
    pub inference: InferenceDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InferenceDefaults {
    #[serde(default = "default_temp")]
    pub temperature: f32,
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    #[serde(default = "default_ctx")]
    pub ctx: u32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_temp() -> f32 { 0.7 }
fn default_top_p() -> f32 { 0.8 }
fn default_top_k() -> u32 { 20 }
fn default_ctx() -> u32 { 32768 }
fn default_max_tokens() -> u32 { 8192 }

impl Default for InferenceDefaults {
    fn default() -> Self {
        Self {
            temperature: default_temp(),
            top_p: default_top_p(),
            top_k: default_top_k(),
            ctx: default_ctx(),
            max_tokens: default_max_tokens(),
        }
    }
}

/// A concrete retrievable model + quant + backend. NOT a blob — a reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelBinding {
    /// The role this binding fills.
    pub role_id: String,
    /// Normalized ref: `qwen3.8-27b@NVFP4` or opencode model id like `zen`.
    pub model_ref: String,
    /// Backend engine: llamacpp | freetoken | ollama | opencode (None = auto).
    #[serde(default)]
    pub engine: Option<String>,
    /// Per-binding overrides serialized as JSON (profile/quant/ctx overrides).
    #[serde(default)]
    pub overrides_json: String,
    /// Active flag; inactive bindings don't resolve.
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_true() -> bool { true }

// ---------------------------------------------------------------- Nodes / Edges

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeKind {
    /// Single-shot generation via `inference::run_prompt` against a live slot.
    Stateless,
    /// Stateful, tool-using agent via an opencode session/TUI.
    Agentic,
    /// Human gate — pauses for approval, does not run an LLM (human-in-the-loop).
    Human,
}

/// A canvas cell: a Role bound to a Model, plus graph metadata + exec overrides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowNode {
    pub id: String,
    pub role_id: String,
    pub binding: ModelBinding,
    pub kind: NodeKind,
    /// Canvas position/size (non-semantic — executor ignores).
    #[serde(default)]
    pub pos: NodePos,
    /// Exec overrides on top of the Role's inference defaults.
    #[serde(default)]
    pub exec: NodeExec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct NodePos {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeExec {
    #[serde(default)]
    pub timeout_s: u32,
    #[serde(default)]
    pub max_tokens: u32,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
}

fn default_retries() -> u32 { 0 }

impl Default for NodeExec {
    fn default() -> Self {
        Self { timeout_s: 0, max_tokens: 0, max_retries: default_retries() }
    }
}

impl WorkflowNode {
    /// A compact, human-readable kind label used by CLI/UI surfacing.
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            NodeKind::Stateless => "stateless",
            NodeKind::Agentic => "agentic",
            NodeKind::Human => "human",
        }
    }
}

/// A serialized predicate string evaluated against a node's output text for
/// conditional routing (Phase 8f). Devoid of code: it is parsed into an
/// [`EdgePredicate`], never executed. The string form is the persisted shape on
/// `WorkflowEdge.condition`.
///
/// V1.5 supported forms (case-sensitive on the substring):
///   - `"always"` / empty / missing → always route.
///   - `"contains:<sub>"`        → route when output contains `<sub>`.
///   - `"not_contains:<sub>"`    → route when output does NOT contain `<sub>`.
///   - `"starts_with:<sub>"`     → route when output starts with `<sub>`.
///   - `"is_empty"`              → route when output is blank (trimmed).
///   - `"not_empty"`             → route when output has non-blank content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgePredicate {
    Always,
    Contains(String),
    NotContains(String),
    StartsWith(String),
    IsEmpty,
    NotEmpty,
}

impl EdgePredicate {
    /// Parse a serialized predicate string. Returns `Always` for missing/empty
    /// input (unconditional edge). Errors on an unrecognized operator.
    pub fn parse(raw: &str) -> Result<EdgePredicate, String> {
        let s = raw.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("always") {
            return Ok(EdgePredicate::Always);
        }
        let (op, arg) = match s.split_once(':') {
            Some((o, a)) => (o, Some(a)),
            None => (s, None),
        };
        match (op, arg) {
            ("contains", Some(a)) => Ok(EdgePredicate::Contains(a.to_string())),
            ("not_contains", Some(a)) => Ok(EdgePredicate::NotContains(a.to_string())),
            ("starts_with", Some(a)) => Ok(EdgePredicate::StartsWith(a.to_string())),
            ("is_empty", None) => Ok(EdgePredicate::IsEmpty),
            ("not_empty", None) => Ok(EdgePredicate::NotEmpty),
            _ => Err(format!("unknown edge predicate '{raw}'")),
        }
    }

    /// Evaluate the predicate against a node's produced output text.
    pub fn eval(&self, text: &str) -> bool {
        match self {
            EdgePredicate::Always => true,
            EdgePredicate::Contains(sub) => text.contains(sub),
            EdgePredicate::NotContains(sub) => !text.contains(sub),
            EdgePredicate::StartsWith(sub) => text.starts_with(sub),
            EdgePredicate::IsEmpty => text.trim().is_empty(),
            EdgePredicate::NotEmpty => !text.trim().is_empty(),
        }
    }
}

/// A directed connection `from → to`. The `condition` (Phase 8f) is a serialized
/// predicate that routes only when the producer's output satisfies it; the
/// `loop_edge` flag marks a bounded back-edge that closes a loop body (never a
/// raw cycle — the scheduler validates this).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    /// Which port of `from` feeds which port of `to` (V1: both "out"/"in").
    #[serde(default)]
    pub from_port: String,
    #[serde(default)]
    pub to_port: String,
    /// Serialized predicate for conditional routing (Phase 8f). `None`/empty =
    /// unconditional (backward compatible with V1 documents).
    #[serde(default)]
    pub condition: Option<String>,
    /// When true, this edge is a bounded back-edge closing a loop body (Phase
    /// 8f). The scheduler only permits back-edges to close cycles, never a raw
    /// cyclic graph, and `max_iterations` (0 = none) bounds the body.
    #[serde(default)]
    pub loop_edge: bool,
}

// ---------------------------------------------------------------- Workflow

/// Execution budget / stop policy — loop safeguards live here even though loops
/// are V2, so a future loop construct is already policed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecSettings {
    #[serde(default = "default_max_parallel")]
    pub max_parallel: u32,
    #[serde(default = "default_retries")]
    pub global_retries: u32,
    #[serde(default)]
    pub budget_tokens: u64,
    #[serde(default)]
    pub budget_wall_s: u64,
    /// Maximum loops a loop-construct body may iterate (0 = no loops allowed).
    #[serde(default)]
    pub max_iterations: u32,
}

fn default_max_parallel() -> u32 { 2 }

impl Default for ExecSettings {
    fn default() -> Self {
        Self {
            max_parallel: default_max_parallel(),
            global_retries: default_retries(),
            budget_tokens: 0,
            budget_wall_s: 0,
            max_iterations: 0,
        }
    }
}

/// A versioned, serializable workflow document. JSON is the source of truth for
/// the graph; positions are UI metadata stored alongside (non-semantic).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
    #[serde(default)]
    pub exec_settings: ExecSettings,
    #[serde(default)]
    pub template: bool,
    /// Kickoff inputs templated into the first wave (CrewAI `inputs`, Studio `Input`).
    /// The canonical key is `task`; extra keys are future-facing.
    #[serde(default)]
    pub inputs: HashMap<String, String>,
}

fn default_version() -> u32 { 1 }

impl Workflow {
    /// `true` if the graph contains a cycle **not** closed by a loop back-edge.
    /// Loop back-edges (`edge.loop_edge`) are the only permitted way to close a
    /// cycle (Phase 8f); this predicate reports a genuine, invalid raw cycle.
    pub fn has_cycle(&self) -> bool {
        has_unpermitted_cycle(self)
    }

    /// The single loop back-edge (Phase 8f), if any. V1.5 supports exactly one
    /// bounded loop; `validate` rejects workflows with more.
    pub fn loop_edges(&self) -> impl Iterator<Item = &WorkflowEdge> {
        self.edges.iter().filter(|e| e.loop_edge)
    }

    /// Structural validation for Phase 8f: no raw cycles, loop back-edges are
    /// well-formed (≤ 1, must close a cycle, need `max_iterations >= 1`), and
    /// every edge's condition parses. Returns the first problem found.
    pub fn validate(&self) -> Result<(), String> {
        if has_unpermitted_cycle(self) {
            return Err("workflow has a raw cycle (only a loop back-edge may close a cycle)".into());
        }
        let loop_edges: Vec<&WorkflowEdge> = self.edges.iter().filter(|e| e.loop_edge).collect();
        if loop_edges.len() > 1 {
            return Err("V1.5 supports at most one loop back-edge".into());
        }
        if let Some(le) = loop_edges.first() {
            if self.exec_settings.max_iterations == 0 {
                return Err("loop back-edge present but exec_settings.max_iterations is 0 (loops disabled)".into());
            }
            // Loop edge must not also be self-flagged uselessly, and must connect real nodes.
            if !self.nodes.iter().any(|n| n.id == le.from)
                || !self.nodes.iter().any(|n| n.id == le.to)
            {
                return Err(format!("loop edge '{}' references unknown node", le.id));
            }
        }
        for e in &self.edges {
            if let Some(c) = &e.condition {
                EdgePredicate::parse(c)
                    .map_err(|msg| format!("edge '{}': {msg}", e.id))?;
            }
        }
        Ok(())
    }
}

/// Pure cycle check over the graph with loop back-edges removed. Returns `true`
/// when a cycle that is NOT closed by a loop back-edge exists (an invalid graph).
fn has_unpermitted_cycle(wf: &Workflow) -> bool {
    let mut indeg: HashMap<String, usize> = HashMap::new();
    for n in &wf.nodes {
        indeg.entry(n.id.clone()).or_insert(0);
    }
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for e in &wf.edges {
        if e.loop_edge {
            continue; // loop back-edges never count toward a raw cycle
        }
        adj.entry(e.from.clone()).or_default().push(e.to.clone());
        *indeg.entry(e.to.clone()).or_insert(0) += 1;
    }
    // Kahn's algorithm — if we can't empty the queue, a cycle exists.
    let mut q: Vec<String> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut seen = 0usize;
    while let Some(u) = q.pop() {
        seen += 1;
        if let Some(next) = adj.get(&u) {
            for v in next {
                if let Some(d) = indeg.get_mut(v) {
                    *d -= 1;
                    if *d == 0 {
                        q.push(v.clone());
                    }
                }
            }
        }
    }
    seen != wf.nodes.len()
}

// ---------------------------------------------------------------- Messages

/// The typed unit passed across an edge. V1: text + optional structured JSON.
/// File/artifact refs are supported by reference so messages stay small.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    /// Producer node_run id.
    pub node_run_id: String,
    pub kind: MessageKind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub structured: Option<serde_json::Value>,
    /// A produced file/artifact path (patch, report, code).
    #[serde(default)]
    pub ref_path: Option<String>,
    #[serde(default)]
    pub meta: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageKind {
    Text,
    Structured,
    FileRef,
    ArtifactRef,
}

// ---------------------------------------------------------------- Runs

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkflowRunStatus {
    Queued,
    Running,
    Done,
    Partial,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeRunStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

// ---------------------------------------------------------------- DAG Scheduler

/// A schedule of "ready node" waves. Each wave is a set of node ids that may
/// run concurrently; all nodes in wave 0 have no unsatisfied upstream inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryPlan {
    /// Ordered waves (index 0 first). Nodes within a wave are independent.
    pub waves: Vec<Vec<String>>,
    /// `node_id → set of direct predecessor node ids`.
    pub predecessors: HashMap<String, Vec<String>>,
    /// `node_id → set of direct successor node ids`.
    pub successors: HashMap<String, Vec<String>>,
    /// Node ids that are unreachable (no path from any source) — safe to warn.
    pub unreachable: Vec<String>,
    /// The single bounded loop back-edge (Phase 8f), if any. The executor re-runs
    /// the body while its (continue) predicate holds, bounded by `max_iterations`
    /// and the token budget.
    pub loop_back_edge: Option<WorkflowEdge>,
}

/// Pure topological plan for a workflow. Refuses raw cycles; loop back-edges
/// are excluded from the body topology (so the graph's forward skeleton is a
/// DAG) and reported to the executor separately.
///
/// Fan-in semantics (V1): a node is "ready" only when **all** its predecessors
/// have produced output (conjunction). A fan-out simply copies the source's
/// message to every downstream edge. Waves are the maximal sets of nodes whose
/// inputs are all satisfied and independent, greedy-packed subject to no cross
/// dependency within a wave.
///
/// Phase 8f: `plan` is branch-aware in that conditional edges still count as
/// predecessors for wave ordering (the topology is static); deciding whether a
/// downstream node actually runs/skips happens at execution time against the
/// produced text. The returned `loop_back_edge` names the bounded loop (if any).
pub fn plan(wf: &Workflow) -> Result<DryPlan, String> {
    if has_unpermitted_cycle(wf) {
        return Err("workflow has a raw cycle; only a loop back-edge may close a cycle".into());
    }
    let loop_back_edge = wf.loop_edges().next().cloned();
    // adjacency over the forward skeleton (loop back-edges excluded)
    let mut pred: HashMap<String, Vec<String>> = HashMap::new();
    let mut succ: HashMap<String, Vec<String>> = HashMap::new();
    for n in &wf.nodes {
        pred.entry(n.id.clone()).or_default();
        succ.entry(n.id.clone()).or_default();
    }
    for e in &wf.edges {
        if e.loop_edge {
            continue;
        }
        succ.entry(e.from.clone()).or_default().push(e.to.clone());
        pred.entry(e.to.clone()).or_default().push(e.from.clone());
    }

    // indegrees for wave generation
    let mut indeg: HashMap<String, usize> = wf
        .nodes
        .iter()
        .map(|n| (n.id.clone(), pred.get(&n.id).map(|p| p.len()).unwrap_or(0)))
        .collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut waves: Vec<Vec<String>> = Vec::new();

    loop {
        let ready: Vec<String> = wf
            .nodes
            .iter()
            .filter(|n| !visited.contains(&n.id))
            .filter(|n| indeg.get(&n.id).copied().unwrap_or(0) == 0)
            .map(|n| n.id.clone())
            .collect();
        if ready.is_empty() {
            break;
        }
        for id in &ready {
            visited.insert(id.clone());
            if let Some(next) = succ.get(id) {
                for v in next {
                    if let Some(d) = indeg.get_mut(v) {
                        *d -= 1;
                    }
                }
            }
        }
        waves.push(ready);
    }

    // any node not visited is unreachable (isolated or sink that never got a
    // path cleared — after Kahn this means it was never enqueued)
    let unreachable: Vec<String> = wf
        .nodes
        .iter()
        .map(|n| n.id.clone())
        .filter(|id| !visited.contains(id))
        .collect();

    Ok(DryPlan {
        waves,
        predecessors: pred,
        successors: succ,
        unreachable,
        loop_back_edge,
    })
}

// ---------------------------------------------------------------- Seeds

/// Seed roles the first workflow templates use. These are examples the user can
/// edit/rename; they only exist to make "Coding Review" and "Debate" runnable on
/// first use.
pub fn seed_coding_review_roles() -> Vec<Role> {
    vec![
        Role {
            id: "primary-developer".into(),
            name: "Primary Developer".into(),
            description: "writes the code / patch against the working tree".into(),
            system_prompt: "You are a senior software engineer. Given the task, write or modify code and produce a clear summary of what changed and any patch/diff.".into(),
            instructions: "Work against the repository directory. Output a description of changes plus the diff when applicable.".into(),
            input_contract: "task description".into(),
            output_contract: "text with change summary + optional patch".into(),
            tools: vec!["read".into(), "edit".into(), "shell".into(), "git".into()],
            inference: InferenceDefaults::default(),
        },
        Role {
            id: "architecture-reviewer".into(),
            name: "Architecture Reviewer".into(),
            description: "reviews the developer's output for design/architecture issues".into(),
            system_prompt: "You are a careful architecture reviewer. Read the proposed change and critique design, correctness, and maintainability. Be specific and constructive.".into(),
            instructions: "Given the developer's summary/patch, list issues by severity and recommend concrete fixes.".into(),
            input_contract: "developer output (summary + patch)".into(),
            output_contract: "structured critique (issues + recommendations)".into(),
            tools: vec!["read".into()],
            inference: InferenceDefaults { temperature: 0.3, ctx: 32768, max_tokens: 4096, ..Default::default() },
        },
    ]
}

/// Build the "Coding Review" linear template (V1: Dev → Reviewer, unconditional).
pub fn seed_coding_review() -> Workflow {
    Workflow {
        id: "coding-review".into(),
        name: "Coding Review".into(),
        description: "LV1: developer writes, reviewer critiques. Linear DAG.".into(),
        version: 1,
        nodes: vec![
            WorkflowNode {
                id: "n1".into(),
                role_id: "primary-developer".into(),
                binding: ModelBinding { role_id: "primary-developer".into(), model_ref: "qwen3.8-27b@Q3_K_XL".into(), engine: Some("llamacpp".into()), overrides_json: String::new(), active: true },
                kind: NodeKind::Agentic,
                pos: NodePos { x: 40.0, y: 120.0 },
                exec: NodeExec::default(),
            },
            WorkflowNode {
                id: "n2".into(),
                role_id: "architecture-reviewer".into(),
                binding: ModelBinding { role_id: "architecture-reviewer".into(), model_ref: "qwen3.6-35b-a3b@NVFP4".into(), engine: Some("freetoken".into()), overrides_json: String::new(), active: true },
                kind: NodeKind::Stateless,
                pos: NodePos { x: 40.0, y: 320.0 },
                exec: NodeExec::default(),
            },
        ],
        edges: vec![WorkflowEdge { id: "e1".into(), from: "n1".into(), to: "n2".into(), from_port: "output".into(), to_port: "input".into(), condition: None, loop_edge: false }],
        exec_settings: ExecSettings::default(),
        template: true,
            inputs: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_plans_to_two_waves() {
        let wf = seed_coding_review();
        assert!(!wf.has_cycle());
        let p = plan(&wf).expect("plan");
        // n1 (developer) runs first, then n2 (reviewer)
        assert_eq!(p.waves.len(), 2);
        assert_eq!(p.waves[0], vec!["n1"]);
        assert_eq!(p.waves[1], vec!["n2"]);
        assert!(p.predecessors["n2"].contains(&"n1".to_string()));
        assert!(p.successors["n1"].contains(&"n2".to_string()));
        assert!(p.unreachable.is_empty());
    }

    #[test]
    fn fan_out_fan_in_parallel_wave() {
        // A → B, A → C, B → D, C → D  → waves: [A], [B,C], [D]
        let wf = Workflow {
            id: "debate".into(),
            name: "Debate".into(),
            description: String::new(),
            version: 1,
            nodes: vec![
                WorkflowNode { id: "a".into(), role_id: "p".into(), binding: binding("a"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "b".into(), role_id: "p".into(), binding: binding("b"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "c".into(), role_id: "p".into(), binding: binding("c"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "d".into(), role_id: "p".into(), binding: binding("d"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
            ],
            edges: vec![
                e("ea", "a", "b"), e("eb", "a", "c"), e("ec", "b", "d"), e("ed", "c", "d"),
            ],
            exec_settings: ExecSettings::default(),
            template: true,
            inputs: Default::default(),
        };
        let p = plan(&wf).expect("plan");
        assert_eq!(p.waves.len(), 3);
        assert_eq!(p.waves[0], vec!["a"]);
        // B and C are independent → same wave
        let second = &p.waves[1];
        assert_eq!(second.len(), 2);
        assert!(second.contains(&"b".to_string()) && second.contains(&"c".to_string()));
        assert_eq!(p.waves[2], vec!["d"]);
    }

    #[test]
    fn rejects_cycles() {
        let wf = Workflow {
            id: "loop".into(),
            name: "Loop".into(),
            description: String::new(),
            version: 1,
            nodes: vec![
                WorkflowNode { id: "a".into(), role_id: "p".into(), binding: binding("a"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "b".into(), role_id: "p".into(), binding: binding("b"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
            ],
            edges: vec![e("e1", "a", "b"), e("e2", "b", "a")],
            exec_settings: ExecSettings::default(),
            template: false,
            inputs: Default::default(),
        };
        assert!(wf.has_cycle());
        assert!(plan(&wf).is_err());
    }

    fn binding(role: &str) -> ModelBinding {
        ModelBinding { role_id: role.into(), model_ref: "x@Q4".into(), engine: None, overrides_json: String::new(), active: true }
    }
    fn e(id: &str, from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { id: id.into(), from: from.into(), to: to.into(), from_port: "output".into(), to_port: "input".into(), condition: None, loop_edge: false }
    }

    #[test]
    fn predicate_parse_roundtrip() {
        assert_eq!(EdgePredicate::parse("").unwrap(), EdgePredicate::Always);
        assert_eq!(EdgePredicate::parse("always").unwrap(), EdgePredicate::Always);
        assert_eq!(
            EdgePredicate::parse("contains:patch").unwrap(),
            EdgePredicate::Contains("patch".into())
        );
        assert_eq!(
            EdgePredicate::parse("not_contains:ERR").unwrap(),
            EdgePredicate::NotContains("ERR".into())
        );
        assert_eq!(
            EdgePredicate::parse("starts_with:OK").unwrap(),
            EdgePredicate::StartsWith("OK".into())
        );
        assert_eq!(EdgePredicate::parse("is_empty").unwrap(), EdgePredicate::IsEmpty);
        assert_eq!(EdgePredicate::parse("not_empty").unwrap(), EdgePredicate::NotEmpty);
        assert!(EdgePredicate::parse("bogus:x").is_err());
    }

    #[test]
    fn predicate_eval_routes_on_output_text() {
        assert!(EdgePredicate::Always.eval("anything"));
        assert!(EdgePredicate::Contains("patch".into()).eval("here is a patch"));
        assert!(!EdgePredicate::Contains("patch".into()).eval("no diff here"));
        assert!(EdgePredicate::NotContains("ERR".into()).eval("all good"));
        assert!(!EdgePredicate::NotContains("ERR".into()).eval("ERR: failed"));
        assert!(EdgePredicate::StartsWith("OK".into()).eval("OK done"));
        assert!(!EdgePredicate::StartsWith("OK".into()).eval("no"));
        assert!(EdgePredicate::IsEmpty.eval("   \n "));
        assert!(!EdgePredicate::IsEmpty.eval("text"));
        assert!(EdgePredicate::NotEmpty.eval("text"));
        assert!(!EdgePredicate::NotEmpty.eval(""));
    }

    #[test]
    fn loop_back_edge_allows_bounded_cycle() {
        // n1 -> n2 plus a loop back-edge n2 -> n1 -> forms a closed body.
        // Because the back-edge is a loop_edge, this is a PERMITTED cycle, not a
        // raw one — plan() must accept it and expose it, while has_cycle is false.
        let wf = Workflow {
            id: "review-loop".into(),
            name: "Review Loop".into(),
            description: String::new(),
            version: 1,
            nodes: vec![
                WorkflowNode { id: "dev".into(), role_id: "p".into(), binding: binding("dev"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "rev".into(), role_id: "p".into(), binding: binding("rev"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
            ],
            edges: vec![
                e_with("e1", "dev", "rev", None, false),
                e_with("e2", "rev", "dev", Some("not_contains:DONE".into()), true),
            ],
            exec_settings: ExecSettings { max_iterations: 4, ..Default::default() },
            template: false,
            inputs: Default::default(),
        };
        assert!(!wf.has_cycle(), "loop back-edge must not report a raw cycle");
        assert_eq!(wf.validate(), Ok(()));
        let p = plan(&wf).expect("plan with a loop back-edge accepted");
        // forward skeleton is a DAG: dev -> rev
        assert_eq!(p.waves[0], vec!["dev"]);
        assert_eq!(p.waves[1], vec!["rev"]);
        let back = p.loop_back_edge.as_ref().expect("loop edge surfaced");
        assert_eq!(back.from, "rev");
        assert_eq!(back.to, "dev");
    }

    #[test]
    fn raw_cycle_without_loop_edge_rejected() {
        let wf = Workflow {
            id: "bad".into(),
            name: "Bad".into(),
            description: String::new(),
            version: 1,
            nodes: vec![
                WorkflowNode { id: "a".into(), role_id: "p".into(), binding: binding("a"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "b".into(), role_id: "p".into(), binding: binding("b"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
            ],
            edges: vec![e("e1", "a", "b"), e("e2", "b", "a")],
            exec_settings: ExecSettings::default(),
            template: false,
            inputs: Default::default(),
        };
        assert!(wf.has_cycle());
        assert!(plan(&wf).is_err());
        assert!(wf.validate().is_err());
    }

    #[test]
    fn loop_edge_requires_max_iterations() {
        let wf = Workflow {
            id: "loop-nobudget".into(),
            name: "Loop no budget".into(),
            description: String::new(),
            version: 1,
            nodes: vec![
                WorkflowNode { id: "a".into(), role_id: "p".into(), binding: binding("a"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
                WorkflowNode { id: "b".into(), role_id: "p".into(), binding: binding("b"), kind: NodeKind::Stateless, pos: NodePos::default(), exec: NodeExec::default() },
            ],
            edges: vec![e("e1", "a", "b"), e_with("e2", "b", "a", None, true)],
            exec_settings: ExecSettings::default(), // max_iterations == 0
            template: false,
            inputs: Default::default(),
        };
        assert!(!wf.has_cycle());
        assert!(wf.validate().is_err(), "loop with max_iterations 0 must be rejected");
    }

    fn e_with(id: &str, from: &str, to: &str, condition: Option<String>, loop_edge: bool) -> WorkflowEdge {
        WorkflowEdge { id: id.into(), from: from.into(), to: to.into(), from_port: "output".into(), to_port: "input".into(), condition, loop_edge }
    }
}
