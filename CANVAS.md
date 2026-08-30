# Infinite Agent Canvas — Architecture Proposal

Status: **Design proposal for review — no code in this doc is implemented.**
Date: 2026-08-30
Scope: research + roadmap planning only. Grounded in a close read of the
existing `cyberdeck` codebase (crates, SQLite schema, frontend stores/views,
ROADMAP.md Phase 8 draft).

---

## 0. TL;DR

The Infinite Agent Canvas is **two separable things** that happen to share a
name: (1) a *visual editor* for agent workflows, and (2) a *workflow execution +
measurement* engine. The UI is the thin shell; the durable value is the
execution model and the **Role ↔ Model** separation that lets Cyberdeck
accumulate "which model is good at which role" intelligence over time.

The core design decision — **separate the Role from the Model** — is confirmed.
A saved workflow is a graph of **Roles**, each Role bound to a **named Model
binding** (family + version + quant), so a workflow is reproducible AND the same
Role can be re-run/benchmarked across many models without editing the graph.

Everything needed already exists as primitives:
- `deck-engines::inference::run_prompt` (stateless single-shot generation)
- `deck-tauri::console` opencode multiplexer + `tui.rs` PTY embed (stateful agentic TUI)
- `deck-engines::evaluation::Evaluator` trait (Exact/Regex/JsonSchema/LmEval)
- `matrix_runs` + `evaluations` + `hardware_profiles` + `workloads` + `recommend`
  (the measurement + per-role intelligence substrate)
- the `Profile` loadout / resident port map / bring-up path (how a node gets a live backend)
- the module-store frontend pattern (`subscribe`/`getSnapshot`/`bump` + `useSyncExternalStore`)

The proposal builds a **data-driven, serializable Workflow** (JSON, stored in
SQLite) + a **deterministic graph scheduler** in `deck-core`/`deck-engines`,
and a `CANVAS` top-level view using the existing module-store + Tauri-event
pattern. V1 is deliberately small: **linear + parallel fan-out/fan-in DAGs
(no loops, no conditionals, no supervisor)**, with explicit extension points for
the rest. The next milestone is **Workflow Foundation** — the Role/Model data
model + serializable Workflow + a trivial 2-node executor — NOT the visual
canvas.

---

## 1. Current Cyberdeck Architecture Relevant to This Feature

### 1.1 Crate boundaries (strict, per AGENTS.md)

| Crate | Responsibility | How the canvas maps |
|-------|----------------|---------------------|
| `deck-core` | Pure domain: SQLite store, parsers, fit, workloads, hardware, settings/audit, recommend, relevance | **Owns the Workflow/Role/Model/data model + serialization + DRY graph scheduler.** No I/O beyond the store. |
| `deck-engines` | Engine lifecycle (systemd units), harness (`matrix`/`compare`/`grid`), `inference::run_prompt`, `evaluation::Evaluator` trait | **Owns node execution** (call a live backend), per-node evaluation, fan-out concurrency. |
| `deck-feeds` | Network adapters (HF/GitHub), downloads | Feeds model discovery that later becomes Role candidates. |
| `deck-tauri` | Glue: commands, event emitters, `console` opencode sessions, `tui.rs` PTY | Owns the running-session registry (`workflow_run`), emits `wf-*` events. |
| `deck-cli` | Terminal door over the cores | `deck workflow list/run/stop/save`. |
| `src-tauri` | Thin binary entry | Registers `workflow_*` commands — serialization only. |
| `frontend` | React + `api.ts` invoke + module stores | `CANVAS` view + `canvas` store. UI never shells out; `lib/` never touches DOM. |

**Boundary rule reused verbatim:** business logic lives in the cores; Tauri is
serialization only; the frontend calls `invoke` and consumes Tauri events. The
canvas must not become its own service or bypass this.

### 1.2 Persistence model (deck-core/src/store.rs) — what exists

Single SQLite DB at `~/.local/share/cyberdeck/cyberdeck.db`, opened via
`Connection::open` + additive `CREATE TABLE IF NOT EXISTS` per helper, plus
`ensure_column` (ALTER TABLE ADD COLUMN IF NOT EXISTS) for additive migrations.
**There is no `schema_version` table yet** (`Phase 0` lists adding one).

Relevant tables:

- `models` — path-keyed inventory of local models (`path UNIQUE`, `format`,
  `name`, `arch`, `quant`, `params`, `n_layers`, `ctx_train`, `weight_size`,
  `footprint`). **No model revision/hash column.**
- `profiles` — `name`, `engine`, `body` (JSON). The **`Profile` is a fully
  specified engine launch** (bin, model path, every flag, ctx ladder, port,
  sampling, resource cgroup limits). Port map: `llamacpp :18000`, `freetoken
  :1919`, `ollama :11434`; test ports `:18999/:18998/:18997`.
- `residents` — `engine_id → profile_name, resident` (which profile is bound
  to which slot).
- `matrix_runs` — the measurement record: `engine, model, ctx, task, run,
  verdict (RUNNING/OOM/CRASH/TIMEOUT/ERROR/…), gen_tokens, prompt_tokens,
  tok_s, tok_s_kind, wall_ms, output, at` + Phase-1 provenance `workload_id,
  hardware_profile_id, engine_version, prompt_tps, ttft_ms, peak_vram_mb,
  model_rev, sampling_json`.
- `bench` — quick throughput gauge (`tps`) from a live `/metrics`.
- `workloads` — `id, label, description, tasks_json`. **First-class table,
  not a string tag.** Seeded: coding, reasoning, instruction, assistant, agent.
- `hardware_profiles` — content-hash-deduped machine snapshot
  (`gpu, vram_mb, cpu, ram_mb, os, driver, cuda, cyberdeck_ver, engines_json`).
- `evaluations` — `matrix_run_id FK, method (Deterministic|ModelJudge|Human),
  passed, score, details_json, at`.
- `settings` + `audit_log` — typed settings with audited, revertible writes
  (`actor, key, old_json, new_json, reason`).
- `releases` — online catalog (`source, repo, rev` PK).
- `engine_bin` — per-engine executable override.

**Key implication:** the canvas persistence should reuse `matrix_runs` +
`evaluations` + `hardware_profiles` + `workloads` for *measurement*, and add a
small set of *new* additive tables for `roles`, `model_bindings`, `workflows`,
`workflow_runs`, and `node_runs` — rather than re-inventing a benchmark DB.

### 1.3 Model deployment / how a model becomes reachable

- A `Profile` (loadout) → `deck-engines::unit::render_unit` → systemd user unit
  → installed/backed-up/`apply` → resident on a fixed port.
- `bringup` (`deck-tauri::bringup`) derives a config from a model file, walks a
  ctx ladder on a test port, verifies health, then applies — the "LOAD" path.
- Clients reach a model by its **resident port + model id/alias**.
- `deck-engines::inference::run_prompt(engine, host, port, model_id, prompt,
  max_tokens) -> GenSample` is the **synchronous single-shot generation**
  primitive (OpenAI-compat + Ollama `/api/chat`).

**Implication for canvas:** a "model on a node" is really "a Profile/loadout
bound to a slot (or a one-shot run_prompt against a live slot)." The canvas must
not hardcode a model path — it should reference a **ModelBinding** that the
deployment layer resolves to either (a) a resident slot, (b) a brought-up
one-shot, or (c) an opencode/`zen`/provider model.

### 1.4 Agent architecture (current)

- `deck-tauri::console` — an **opencode session multiplexer**: `opencode run
  --dir X -m MODEL prompt`, server assigns `sess-N`, streams `opencode-started/
  output/done` events, `opencode_stop(id)` SIGTERMs one session. Sessions run
  concurrently. This is the *agentic, tool-using* execution primitive.
- `deck-tauri::tui` — the **real opencode PTY embed** (just built, Phase 8a):
  one shared `opencode serve :19771` + `opencode attach --dir` on a PTY per
  pane, streamed as `tui-data` events into `xterm.js`. **This is the per-node
  interactive TUI vehicle.**
- `deck-tauri::agent` — Phase 7a typed tools (READ/ANALYZE/MODIFY/EXECUTE),
  built on `settings` + `audit_log` for safe write-back. Permission ladder
  documented in ROADMAP Phase 7. `opencode` already reaches `zen`/provider
  models via its own config — no extra wiring.

**Implication:** a canvas **Node** can be executed in two modes that already
exist:
- **Stateless/prompt node** → `inference::run_prompt` against a live slot
  (fast, cheap, single-shot, easy to bench per-role).
- **Agentic/stateful node** → an `opencode` session/TUI (tools, files, MCP,
  multi-step). Heavier; per-role bench is harder but still possible via the
  existing output capture + evaluation.

V1 should support **both**, because the distinction is already in the codebase
and it materially changes cost/VRAM/parallelism behavior.

### 1.5 Benchmarking / model intelligence substrate

The measurement→evaluate→aggregate→recommend chain is **already built**:
`matrix_runs` (+ `evaluations`, + `hardware_profiles`, + `workloads`) feed a
deterministic `recommend(workload, objective)` (deck-core/src/recommend.rs).
Today this is **workload-scoped, not role-scoped**. The single biggest canvas →
benchmark integration is to add **role identity** as a first-class dimension to
`matrix_runs`/`evaluations` (or a parallel `node_runs` view), so intelligence
can answer "best model for the *Architecture Reviewer* role" instead of only
"best model for *coding*."

### 1.6 Frontend architecture

- `App.tsx` routes 8 top-level views: `HUD, VAULT, SIGNALS, MARKET, DOWNLOADS,
  LOADOUTS, CONSOLE, BENCH`. A new view = add to `VIEWS` + a render branch.
- Pattern: **module-level store** (`dl.ts`, `br.ts`) with `subscribe/
  getSnapshot/bump`, consumed via `useSyncExternalStore`; Tauri events are the
  only remote update channel. **Explicit "no giant store.ts" rule** — each view
  fetches via `api.ts` and holds its own local state; global-ish state only in a
  targeted store.
- `Hud.tsx` already: concurrent opencode sessions + real embedded `TuiWindow`s
  (draggable/resizable xterm panes) on a canvas-like area.
- UI never shells out; `api.ts` → Tauri `invoke` is the sole door.

**Implication:** the CANVAS store follows the `dl.ts`/`br.ts` module-store
pattern (not Redux/Zustand — that would violate the "no giant store" rule and
add a dependency). `reactflow` is a candidate for node/edge UI but is **not
required for V1** (see §7).

### 1.7 Config / settings

Typed settings (`settings` table) + audit log + undo already exist and are
agent-safe. Canvas-wide preferences (canvas_layout, zoom, theme) belong in
`settings` keyed like `canvas.*`; workflow definitions belong in **their own
tables**, not in `settings` (they're documents, not knobs).

---

## 2. Feature Vision

The Infinite Agent Canvas is Cyberdeck's **visual workspace for composing,
running, and learning from multi-agent workflows** on the user's local fleet
(plus online `opencode` models).

Not a linear-pipeline maker — an **arbitrary directed graph** (DAG → eventually
loops/conditionals):

```
                 → Agent B →
Agent A →                        Agent D
                 → Agent C →
```

A node is **not "a model."** A node is a **Role** with a configurable Model
assigned to it:

```
Architecture Reviewer          ← Role (stable identity)
├── system prompt / role
├── temperature, ctx, tools, perms
└── Model: Qwen3.8-27B (NVFP4) ← ModelBinding (swappable)
    ├── Qwen3.8-27B
    ├── DeepSeek
    └── Nemotron
```

The canvas is where the user **builds and runs** workflows. It is also where
Cyberdeck **learns** — the same Role run across many models accumulates
per-role performance data that feeds the model-intelligence pipeline (which
exists today as workload-scoped `recommend`; Role-scoped intelligence is the
payoff).

The saved artifact ("workflow") is a **versioned, serializable document** that
preserves graph + roles + model bindings + prompts + params + execution
settings, so a workflow is reproducible today and re-runnable tomorrow.

---

## 3. Core Concepts (defined, no more than these)

Only concepts that are genuinely useful. Five-tier hierarchy mirrors the
user's ask but trims to what the architecture needs.

| Concept | Definition | Notes |
|---------|-----------|-------|
| **Role** | A named, stable job description: `name`, `system_prompt`, `instructions`, input/output contract, `tool_permissions`, inference defaults. **Deliberately has no model.** | The reusable, swappable identity. e.g. `Architecture Reviewer`. |
| **Model** | A concrete retrievable model + quant: `family`, `version/rev`, `quant`, `backend_engine` (llamacpp/freetoken/ollama/opencode). References existing `models`/profile/opencode config, **not** a blob copy. | "Qwen3.8 27B NVFP4". |
| **Binding** | The assignment `Role × Model` at a point in time. | `Architecture Reviewer ⟵ Qwen3.8-27B-NVFP4`. The thing that lets you swap models. |
| **Node** | A **Role + Binding** + graph metadata (position, size, label) + execution overrides (timeout, retries, budget). This is the canvas *cell*. | What the user drags onto the canvas. |
| **Connection (Edge)** | A directed, labeled data flow `from → to`, optionally carrying a **condition** (V1: unconditional) and a **port/port name** (which output feeds which input). | Same edge type serves fan-out and fan-in; condition/loop come later. |
| **Workflow** | A versioned document: DAG of Nodes + Edges + workflow-level execution settings (input contract, output contract, budget, stop policy, metadata). **Serializable.** | The saved/loaded artifact. |
| **Run (WorkflowRun)** | One execution of a Workflow: full provenance snapshot (workflow_version, bindings resolved, hardware_profile_id, started/finished, status, per-node results). | The audit trail + bench input. |
| **NodeRun** | One execution of one Node within a Run: input message(s), output message(s), GenSample (tok_s, ttft), status (ok/error/timeout/retry/failed), evaluation rows. | The per-role measurement unit. |
| **Message** | The typed unit passed across an edge: `text` OR `structured` (JSON) OR `file_ref` OR `artifact_ref` + `metadata`. V1: text + optional JSON. | See §6 message model. |
| **Artifact** | A produced object referenced by a node output: file path (patch, report, code), DB row id, or external ref. Stored by reference, not inline. | Keeps messages small; enables file/code passing. |
| **Result** | The workflow's final output: aggregate of terminal-node outputs + overall status + run id. | What "Run" produces. |

Deliberately **not** modeled as concepts: "subagent" (a Role can later spawn
sub-Roles = a nested workflow), "supervisor" (a special Role pattern, not a new
entity), "tool" (declared on a Role, fed to opencode/MCP as config).

---

## 4. Proposed Architecture

### 4.1 Placement

```
deck-core::workflow   (NEW)   Role, ModelBinding, Node, Edge, Workflow, run DTOs,
                              JSON serialization, store fns, DRY graph scheduler
                                    │ consumes
deck-core::store      (extend)  roles / model_bindings / workflows / workflow_runs /
                                node_runs tables (additive); reuse matrix_runs/evaluations
                                    │
deck-engines::workflow (NEW)    Execute a dry plan against live engines:
                                dispatch stateless nodes via inference::run_prompt,
                                agentic nodes via opencode sessions, fan-out with a
                                worker pool, apply evaluators, record node_runs.
                                    │ uses existing
deck-engines::evaluation        Evaluator trait (Exact/Regex/JsonSchema/LmEval)
deck-engines::matrix / compare / grid
deck-tauri::workflow   (NEW)    glue: workflow_list/save/run/stop/status + wf-* events
frontend/src/lib/canvas.ts     module store (subscribe/getSnapshot/bump)
frontend/src/components        WorkflowNode, WorkflowEdge, NodeConfig (xterm via TuiPane)
frontend/src/views/Canvas.tsx  the infinite-canvas view
```

This preserves the crate layering and the "one truth, two doors" rule (CLI +
Tauri both reach `deck-core::workflow` / `deck-engines::workflow`).

### 4.2 The Role/Model separation (the load-bearing decision)

- **Role has no model.** A Role is pure prompt/contract/permission config.
- **Binding** is the bridge row/object `(role_id, model_id, overrides, active_at)`.
- A **saved Workflow stores Bindings**, not raw models, so:
  - swapping models = editing a Binding (or a "deployment variant"), not editing the graph;
  - a Workflow can be re-run across a **Model Matrix** (like matrix_runs today) —
    `Architecture Reviewer × [Qwen3.8, DeepSeek, Nemotron]` — and each combo
    produces NodeRuns that feed per-role intelligence.
- This is exactly the `matrix` mental model Cyberdeck already uses at the model
  level, promoted to the role level.

### 4.3 Two execution modes per node (reuse, don't invent)

| Mode | Backend | When | Bench-ability |
|------|---------|------|---------------|
| **Stateless** | `inference::run_prompt` against a resident/live slot | Pure transform/debate/verify steps; cheap, fast | Easy — exactly like a matrix cell: GenSample → MatrixRun-equivalent |
| **Agentic** | an `opencode` session (console multiplexer) or embedded TUI | Coding/review/search with tools/files | Harder but possible: capture output + gen metrics, run Evaluator |

Both are first-class `Node.kind`. V1 ships both because the primitives exist and
because a tool-less "review" node is far cheaper than spinning an agent.

### 4.4 Data-driven + serializable

Workflows are **documents** (JSON) with a stable schema, versioned. The
executor is a pure function `(workflow_doc, resolved_bindings, input, budget) →
dry_plan`, then `deck-engines::workflow` executes that plan. Persistence is
normative (SQLite stores the JSON + metadata) but the JSON is the source of
truth for the graph — same philosophy as `tasks_json` in `workloads` today.

---

## 5. Data Model

### 5.1 Tables (all additive; reuse `ensure_column`-style migration, no schema_version yet but Phase 0 should add it before these land)

```sql
-- roles: stable job descriptions, model-agnostic
CREATE TABLE IF NOT EXISTS roles (
  id TEXT PRIMARY KEY,            -- slug: 'architecture-reviewer'
  name TEXT NOT NULL,
  description TEXT,
  system_prompt TEXT NOT NULL,
  instructions TEXT,
  input_contract_json TEXT,       -- optional: expected message shape
  output_contract_json TEXT,      -- optional: produced shape
  tools_json TEXT NOT NULL,       -- tool whitelist / permissions (opencode tools, MCP ids)
  inference_defaults_json TEXT,   -- temperature, top_p, top_k, ctx, max_tokens, reasoning
  created_at INTEGER, updated_at INTEGER
);

-- model_bindings: which Role is bound to which Model (+ overrides). The
-- bridge that lets you swap models without touching the graph.
CREATE TABLE IF NOT EXISTS model_bindings (
  id TEXT PRIMARY KEY,
  role_id TEXT NOT NULL,
  model_ref TEXT NOT NULL,        -- normalized: 'qwen3.8-27b@NVFP4' or opencode 'zen'
  engine TEXT,                    -- llamacpp|freetoken|ollama|opencode (NULL=auto)
  overrides_json,                 -- per-binding profile/quant/ctx overrides
  active INTEGER DEFAULT 1,
  created_at INTEGER,
  UNIQUE(role_id, model_ref)
);

-- workflows: versioned, serializable documents
CREATE TABLE IF NOT EXISTS workflows (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT,
  version INTEGER NOT NULL DEFAULT 1,
  graph_json TEXT NOT NULL,       -- nodes[] + edges[] with bindings + positions
  input_contract_json, output_contract_json,
  exec_settings_json,             -- budget, max_parallel, stop policy, retries
  template INTEGER DEFAULT 0,
  created_at INTEGER, updated_at INTEGER
);

-- workflow_runs: one execution; provenance snapshot for bench + audit
CREATE TABLE IF NOT EXISTS workflow_runs (
  id TEXT PRIMARY KEY,
  workflow_id TEXT NOT NULL,
  workflow_version INTEGER NOT NULL,
  graph_snapshot_json TEXT NOT NULL,  -- resolved graph: role+model per node
  hardware_profile_id INTEGER,
  status TEXT NOT NULL,               -- queued|running|done|error|stopped|partial
  input_ref_json, output_ref_json,
  budget_stats_json,
  started_at INTEGER, finished_at INTEGER,
  FOREIGN KEY(hardware_profile_id) REFERENCES hardware_profiles(id)
);

-- node_runs: per-node measurement; the per-role bench unit
CREATE TABLE IF NOT EXISTS node_runs (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,               -- workflow_runs.id
  node_id TEXT NOT NULL,
  role_id TEXT NOT NULL,
  model_ref TEXT NOT NULL,
  kind TEXT NOT NULL,                 -- stateless|agentic
  status TEXT NOT NULL,
  input_ref_json,                     -- message(s) consumed
  output_ref_json,                    -- message(s)/artifact produced
  gen_tokens, prompt_tokens, tok_s, tok_s_kind,
  ttft_ms, wall_ms,
  error_json, retries INTEGER DEFAULT 0,
  matrix_run_id INTEGER,              -- link a stateless node to matrix_runs when applicable
  at INTEGER,
  FOREIGN KEY(matrix_run_id) REFERENCES matrix_runs(id)
);
```

Optional but recommended (so we don't churn `matrix_runs`): a **role_id column
on `matrix_runs`** (additive, `ensure_column`) so per-role bench statistics can
be queried without a parallel DB. This is the cleanest way to make the existing
`recommend` engine eventually role-aware without rewriting it.

### 5.2 Workflow JSON (the source of truth for the graph)

```jsonc
{
  "name": "Coding Review",
  "version": 3,
  "nodes": [
    { "id": "n1", "role_id": "primary-developer",
      "binding": { "model_ref": "qwen3.8-27b@Q3_K_XL", "engine": "llamacpp" },
      "kind": "agentic", "pos": { "x": 40, "y": 120 },
      "exec": { "timeout_s": 600, "max_tokens": 8192 } },
    { "id": "n2", "role_id": "architecture-reviewer",
      "binding": { "model_ref": "qwen3.6-35b-a3b@NVFP4", "engine": "freetoken" },
      "kind": "stateless", "pos": { "x": 40, "y": 320 },
      "exec": { "timeout_s": 120 } }
  ],
  "edges": [ { "id": "e1", "from": "n1", "to": "n2",
               "from_port": "output", "to_port": "input" } ],
  "exec_settings": { "max_parallel": 2, "retries": 1,
                     "max_iterations": 1, "budget_tokens": 100000 }
}
```

Positions/sizes are canvas UI metadata stored in the same doc (so layout is
saved with the workflow, per the ask) but **marked as non-semantic** — the
executor ignores them.

### 5.3 Model reference strategy (Questions 7–9)

- Store a **normalized `model_ref`** (family@quant/rev) — e.g.
  `qwen3.8-27b@NVFP4` — NOT a raw filesystem path, NOT a namespaced `Profile`.
- A `model_resolve(ref, engine?) → Option<Deployment>` maps that ref to a live
  slot (resident profile) or a one-shot load, or an opencode model id.
- **Reproducibility:** the Workflow doc pins `family@quant`; the *resolved*
  deployment (which exact file/rev/engine) is snapshotted into
  `workflow_runs.graph_snapshot_json` at run time. So "what actually ran" is
  always recoverable, even if `~/models` later has a newer quant.
- **Unavailable model handling:** a node whose Binding resolves to nothing is
  **not silently dropped** — the workflow run is `error`/`partial` with a
  precise `NodeRun.error_json` ("model_ref qwen3.8-27b@Q4_K_M not installed;
  get it from VAULT/MARKET"). The canvas marks the node with a badge and offers
  one-click download via the existing `download`/`market` path. (Question 9)

### 5.4 Sharing one model across nodes (Question 10)

A node's Binding may reference a model that is also used by another node. Two
cases:
- **Same live slot** (e.g. both nodes talk to `:18000`): they serialize on the
  same engine context; the scheduler just avoids double-loading. `Profile`
  `parallel` already exists for single-engine batching.
- **Separate slots / engines**: two distinct deployments; scheduler runs them
  as independent reservations.

The scheduler treats "deployment" as a resource: it maintains a
**reservation table** (node → slot) and refuses to exceed hardware budget (see
§6.4). It never assumes one model = one node.

---

## 6. Execution Architecture

### 6.1 Execution phases

```
1. RESOLVE   bindings → deployments (resident slots / one-shot / opencode).
             Fail-fast on unresolvable; report unavailable models.
2. PLAN      topological sort of the DAG → ordered + parallel wavefronts.
             (V1: DAG. detect cycles → error, no execution.)
3. RESERVE   check hardware budget (VRAM/RAM/ports) can fit the max parallel
             wavefront; if not, serialize or warn.
4. EXECUTE   run ready nodes (Wavefront 0 = source nodes; fan-out when all
             upstream inputs arrived → fan-in gate waits).
5. EVALUATE  apply per-role Evaluator to node output(s) (optional, V1.5+).
6. RECORD    write workflow_runs + node_runs (+ matrix_runs link for stateless).
7. STOP/CANCEL/retry/timeout/budget enforcement throughout.
```

### 6.2 Scheduler (DRY, in `deck-core`)

A **pure DAG scheduler** `plan(dry_plan)` lives in `deck-core::workflow`
(testable headless, no I/O): it owns topological sort, ready-set computation,
fan-in gating, and emits a schedule of "ready node" waves. `deck-engines::workflow`
then *executes* each wave against real engines. Keeping the scheduler pure (same
philosophy as `fit`, `render_unit`) is what makes the graph logic testable and
reusable outside the canvas (Question 20).

- **Parallelism:** all ready nodes in a wave run concurrently, bounded by
  `exec_settings.max_parallel` and the hardware reservation table.
- **Fan-out:** a node's single output message is copied to every downstream
  edge (V1 — no per-port fan-in semantics, so each downstream gets the full
  message; ports/transform come later).
- **Fan-in:** a node with multiple inbound edges runs only when **all** are
  satisfied (conjunction), receiving the messages as an ordered list (V1) or
  keyed by `from_port` (later). A `join` note on the node can reduce (concatenate
  / merge JSON) — V1 supports concatenate/max only.

### 6.3 Messages / context propagation (Questions 12, and §Agent-Communication)

V1 Message:
```rust
struct Message {
  id: String,
  node_run_id: String,        // producer
  kind: Kind,                 // Text | Structured(Value) | FileRef(path) | ArtifactRef(id)
  text: Option<String>,
  structured: Option<serde_json::Value>,
  ref_path: Option<String>,   // file/artifact path
  meta: HashMap<String, Value>, // role label, model_ref, latency, provenance
}
```

- **Streaming (12):** stateless nodes are not streaming (single-shot
  `run_prompt`). Agentic nodes stream via the existing `opencode-output`/TUI
  events; a node's live transcript is visible in its canvas panel, but the
  *final* output Message is recorded atomically on completion. V1 does **not**
  stream partial messages across edges — a downstream node runs only on a
  completed upstream node. (Avoids half-finished-text corruption; streaming
  across edges is V2.)
- **Context** is the accumulated `meta` + any `structured`/file refs, which the
  next node's prompt references. No shared global context object in V1.

### 6.4 Concurrency with limited GPU VRAM (Questions 10–11)

- The `hardware_profiles` table already records `vram_mb`. A node's Binding
  carries a **fit footprint** (from `fit.rs`) — the scheduler's reservation
  table sums concurrent footprints and refuses (or serializes) when concurrent
  wavefront demand exceeds available VRAM.
- V1 policy: **serialize overloaded waves** (turn parallelism down) and surface
  a "would exceed VRAM" warning node badge — using the exact `fit`/hardware math
  Cyberdeck already trusts. It never silently launches an OOM.
- This reuses `fit.rs` + `hardware_profiles` + `bringup`; it does not reinvent
  VRAM modeling.

### 6.5 Errors, retries, timeout, cancellation (13, and loop safeguards)

- **Per-node:** `timeout_s`, `max_retries`. On error → mark `NodeRun.failed`,
  record `error_json`; downstream nodes with `on_failure: "block"` (default)
  don't run → run becomes `partial`; a node with `on_failure: "continue"` passes
  a structured error message downstream.
- **Cancellation** is **downward propagation**: `workflow stop` → cancel the
  in-flight wavefront (SIGTERM opencode sessions via `opencode_stop`, kill
  stateless probes) → mark remaining node runs `cancelled` → run `stopped`.
- **Runaway safeguards** (loops are V2, but the budget plumbing ships in V1 so
  loops land on top of it): `exec_settings` carries `max_iterations`,
  `budget_tokens` (sum of consumed tokens across node runs), `budget_wall_s`,
  and a global per-run token counter. Manual stop is always available (top bar +
  CLI `workflow stop`).

### 6.6 Observability (16–17)

- **Events** (reuse the Tauri event idiom): `wf-started`, `wf-node-start`,
  `wf-node-output`, `wf-node-done`, `wf-node-error`, `wf-done`, `wf-error`,
  each carrying `run_id`/`node_id`. Same channel shape as `opencode-*` /
  `tui-data`.
- **Inspection:** `workflow_runs` + `node_runs` are queryable
  (`deck workflow history <id>`, Tauri `workflow_runs`, frontend Run Detail
  panel). A run's `graph_snapshot_json` + each `NodeRun`'s input/output/error/
  metrics + linked `matrix_run_id` give a full postmortem. Replay = re-run with
  the same snapshot bindings.

### 6.7 Loops & conditional branches (14–15) — extension points, not V1

- **Loops** are represented as an explicit `Loop` construct (a subgraph with a
  back-edge + a termination predicate on budget), NOT as a cycle in the raw
  node graph. The scheduler refuses raw cycles in V1 (error), and V1.5+ adds a
  `loop` node type whose body is a nested subgraph — the budget guardrails from
  §6.5 already police it.
- **Conditionals** are an `Edge.condition` (e.g. `output.contains("patch")`) or
  a `Router` node. V1 ships unconditional edges; the Edge type already has a
  `condition: Option<fn-ish serialized predicate>` slot so adding it later
  doesn't change the edge shape (backward compatible).
- The **reviewer-on-condition** example (the user's motivating case) is a V1.5
  feature: `Primary-Developer →condition → Architecture-Reviewer`. The V1
  version (unconditional linear `Dev → Reviewer`) already validates the whole
  Role/Model/measurement stack.

---

## 7. Canvas UX Architecture

### 7.1 View placement (Question: where does it live?)

A **new top-level `CANVAS` view** in `App.tsx` (alongside HUD/VAULT/MARKET/
CONSOLE/BENCH), not a replacement for HUD. Rationale:
- HUD is the *quick* single-agent chat + loaded-models strip (fast, single
  focus).
- CANVAS is the *composition* workspace (multi-agent, semi-permanent layouts).
They share the same underlying primitives (TuiPane/opencode), but serve
different moments. A "project-level workspace" is overkill for a solo local
tool with one root DB; a top-level view is the right granularity and matches
the existing nav idiom.

### 7.2 UI library decision — `reactflow` not required for V1

- V1 needs: place nodes, drag, connect with edges, pan, minimal zoom. That is
  ~300 lines of vanilla positioned divs + pointer handling (the codebase already
  hand-rolls drag via pointer events in `Hud`/`TuiWindow`, and explicitly avoids
  heavyweight state deps). 
- **Recommendation:** start with **no graph library** — hand-rolled pan/zoom/
  node-drag/edge-render (straight lines + arrows for V1) on the module-store
  pattern. Adopt `reactflow` (or `@xyflow/react`) at the **V1.5** boundary when
  multi-select, alignment, minimap, auto-layout, and grouped subgraphs become
  actual needs — it plugs into the same serialized Workflow doc, so it's a
  renderer swap, not a data-model change. This avoids a heavyweight dependency
  in V1 while keeping the escape hatch (mirrors the repo's "no premature ML /
  no giant store" discipline).

### 7.3 Feature tiers (Required / Useful later / Interesting)

**Required (V1):**
- Pan + zoom (wheel/keys), node drag, node create/delete/duplicate, connect
  edges (source→target), edge delete.
- Per-node config panel (Role pick/editor + Model binding pick + exec overrides).
- Save / load workflow (to the `workflows` table), template the 3 seed layouts.
- Run / stop controls; live node status (queued/running/done/error) colored on
  the node; a compact per-node result/transcript panel.
- Single-top-level-node fan-out + fan-in-gate (conjunction) — enough to run the
  seed "debate" and "research" templates with real parallel fan-out.
- Run history list + a run-detail view (per-node results + errors + metrics).

**Useful (V1.5):**
- Undo/redo, multi-select, alignment/distribution, minimap, keyboard shortcuts,
  context menu, notes/sticky comments, auto-layout, condition edges (Router),
  editable edges, per-role bench comparison panel.
- Input/output ports, structured JSON pass-through, file/artifact passing.

**Interesting (V2+ / future):**
- Subgraphs/groups, loop node type, supervisor/worker template, replay,
  live streaming across edges, collaborative cursors, node packages/templates
  marketplace.

### 7.4 State management

A `frontend/src/lib/canvas.ts` module store (same contract as `dl.ts`/`br.ts`:
`subscribe`/`getSnapshot`/`bump`, `useSyncExternalStore` consumption). It holds:
- the in-memory graph (nodes/edges with positions) being edited,
- the saved-workflow list + current workflow id/version,
- the active run id + per-node run status map (fed by `wf-*` events).

It does **not** hold engine state (that's `portmap`/`br`/`dl`) or bench data
(that's `BENCH` view) — the canvas subscribes/forwards. No new framework.

---

## 8. Model / Benchmark Integration

This is the strategic payoff and must be designed in from day one, even though
the *intelligence* is a later milestone.

### 8.1 What the canvas captures so intelligence can be built later

Every `NodeRun` records the minimal sufficient set (see §5.1): identity
(`role_id`, `model_ref`, `node_id`), the resolved deployment snapshot
(`graph_snapshot_json`), latency/throughput (`tok_s`, `ttft_ms`, `wall_ms`),
success/failure (`status`, `error_json`), and links back to `matrix_run_id` for
stateless nodes. Optionally `input_ref_json`/`output_ref_json` for later
re-evaluation.

### 8.2 What belongs to the canvas vs. the existing benchmark system

**Canvas owns:** role identity, model binding, graph topology, workflow/run/
node-run records, per-node success/latency, workflow-level budget/stop.
**Existing system owns (do NOT duplicate):** raw measurement (tok_s/ttft),
deterministic evaluation (`Evaluator`, `evaluations` table), hardware provenance
(`hardware_profiles`), aggregation + recommendation (`recommend`).

The **bridge** is a `role_id` dimension:
- Add `role_id` (additive) to `matrix_runs` (and optionally `bench`), so a
  stateless node's run slams data into the existing pipeline.
- Later, extend `recommend` to accept an optional `role` filter, producing
  per-role leaderboards:

```
Role: Architecture Reviewer
Qwen3.8   Quality 8.7  Speed 142 tok/s  Success 91%
Nemotron  Quality 8.9  Speed 108 tok/s  Success 94%
DeepSeek  Quality 8.4  Speed 155 tok/s  Success 88%
```

- Per-role aggregate = `node_runs GROUP BY role_id, model_ref` → success_rate +
  mean quality + P50 tok_s. This is the workload `recommend` pattern promoted
  to roles.

### 8.3 Model discovery → role candidates

`releases`/`feeds_rank` already rank new models by hardware relevance. The
canvas surfaces a "try as this Role" affordance on a Role's binding picker:
*vacant role + candidate model from feeds → offer a Role-matrix bench* (V1.5+).
No new discovery system; it reuses feeds + fit + recommend.

### 8.4 Hardware awareness

`workflow_runs` snapshots `hardware_profile_id`. A Role's per-model fit uses
`fit.rs`. The benchmark integration inherits `hardware_profiles` provenance for
free. (See §6.4 for live-reservation, not just bench-time, awareness.)

---

## 9. Roadmap Changes

The current ROADMAP **already has Phase 8 "Canvas & Workflow Orchestration"**
(8a embedded TUIs done; 8b reactflow CANVAS + workflows table sketched). This
proposal **refines and splits** that phase rather than bolting on a new one.
The key correction: **Phase 8b as written jumps straight to the visual canvas
before the workflow *domain* exists.** The correct dependency order is
domain → executor → UI.

### 9.1 New items (inserted as sub-phases of Phase 8)

**8c — Workflow Foundation (P1, ~2–4 days) — NEW, and the next milestone.**
- *Purpose:* the Role/Model/Workflow/Run/NodeRun data model, serialization, the
  pure DAG scheduler, and a minimal **non-UI** executor (2-node linear via
  `run_prompt` + one opencode session) reachable from CLI + Tauri.
- *Depends on:* Phase 0 (`schema_version`), Phase 1 provenance, Phase 2
  workloads + `Evaluator`, Phase 3 hardware, Phase 4 recommend (already P0–P1).
- *Enables:* everything canvas thereafter; per-role measurement begins immediately.
- *Touches:* `deck-core::workflow` (new), `deck-core::store` (roles/bindings/
  workflows/runs tables; `role_id` on `matrix_runs`), `deck-engines::workflow`
  (new executor), `deck-cli` (`deck workflow {save,list,run,stop,history}`),
  `deck-tauri::workflow` (glue + `wf-*` events).
- *Classification:* **Foundational.**
- *DoD:* `deck workflow run` executes a 2-node Role-bound graph against a live
  slot and an opencode session; node_runs + workflow_runs recorded; `hist` shows
  them; `cargo test` green.

**8d — Canvas UI shell (P1, ~3–5 days) — NEW.**
- *Purpose:* the `CANVAS` top-level view: pan/zoom, node drag, edges, node config
  (Role + Binding), save/load, run/stop, per-node status/result, run history.
- *Depends on:* 8c (the domain exists and is testable headless), Phase 7a
  `agent_tools` (so a Role's tools surface surfaces cleanly).
- *Enables:* the visible "spawn reviewer on qwen3.6, coder on qwen3.8, run" demo.
- *Touches:* `frontend/src/views/Canvas.tsx`, `frontend/src/lib/canvas.ts`,
  `frontend/src/App.tsx` (add CANVAS to VIEWS), api.ts.
- *Classification:* **Feature-level** (foundational to the feature's *visibility*,
  but the domain is the foundation).
- *DoD:* user builds the "Coding Review" template on the canvas, saves it, loads
  it, and runs it with two live nodes, watching per-node status.

**8e — Workflow model matrix + per-role bench (P2, ~2–3 days) — NEW.**
- *Purpose:* run one Workflow across a Matrix of Model bindings; aggregate
  per-role leaderboards; feed `recommend`.
- *Depends on:* 8c, Phase 4 `recommend` (role-aware extension).
- *Enables:* the "Architecture Reviewer × [Qwen3.8, DeepSeek, Nemotron]"
  intelligence payoff — the original motivation.
- *Touches:* `deck-engines::workflow` (matrix over bindings), `matrix_runs`
  `role_id` aggregation, `recommend` role filter, CANVAS compare panel.
- *Classification:* **Feature-level** (toward Foundational for the intelligence goal).

**8f — Branch/Supervisor/Workflow-polish (P2, ~3–5 days) — NEW (V1.5).**
- *Purpose:* conditional edges (Router), loop node type with budget guards, a
  supervisor/worker template (reuses Role nesting). Includes UX polish:
  undo/redo, minimap, multi-select, notes.
- *Depends on:* 8d (canvas), 8c (scheduler extension).
- *Enables:* arbitrary graphs (the user's `Dev → Reviewer ⟲` loop), supervisor
  architectures.
- *Touches:* `deck-core::workflow` (loop construct, Edge.condition, Router),
  `deck-engines::workflow` (loop + branch exec), CANVAS.
- *Classification:* **Feature-level / polish.**
- *DoD:* the "Coding Review" loop template runs with a termination predicate and
  a token budget enforced.

### 9.2 Modifications to existing items

- **Phase 8 spit-and-reorder:** keep **8a (embedded TUIs — DONE)** as-is;
  **rewrite 8b** from "reactflow CANVAS + workflows table" into the sequence
  **8c → 8d → 8e → 8f** above. 8b's original impulse (reactflow CANVAS) becomes
  the 8d shell; the "workflows table" becomes 8c. **Do not build reactflow
  before 8c** — the domain must exist first.
- **`deck-core::store` Refactoring Plan** (already lists `store.rs` growth):
  add the new tables under a `workflow` submodule in the same split; keep the
  single `Connection::open` + `ensure_*` aggregator.
- **Phase 0 (schema_version):** this proposal *requires* it before 8c to version
  workflow docs safely. Fold `role_id` on `matrix_runs` into Phase 1's additive
  migration list rather than a separate migration.
- **Phase 9 (Autonomous Daily Driver):** the overnight auto-benching +
  regression logic should later target **roles+workflows** (auto-run a saved
  workflow across new quants → per-role regression), but that is P3 and out of
  scope now — just keep the node_runs schema capable of it.

### 9.3 Dependency chain

```
Phase 0 (schema_version) ─┐
Phase 1 (provenance, role_id on matrix_runs) ─┴─→ 8c Workflow Foundation
Phase 2 (workloads + Evaluator) ────────────────────┘
Phase 3 (hardware) ─────────────────────────────────┘
Phase 4 (recommend) ─────────────────────────────→ 8e role-aware recommend
8c ──────────────────────→ 8d Canvas UI shell ──→ 8f branch/loop/supervisor
8c + Phase 4 ────────────→ 8e model matrix / per-role bench
8a (embedded TUIs, DONE) ─ provides the node TUI vehicle for 8d
```

### 9.4 Priority matrix impact

Add:
- `8c Workflow Foundation (role/model/workflow model + executor)` — P1, Foundational
- `8d Canvas UI shell` — P1, Feature
- `8e Workflow model matrix / per-role bench` — P2, Feature
- `8f Branch/Loop/Supervisor` — P2, Feature/Polish

Update: `Phase 8 Canvas — draggable TUIs (8a)` stays P1/DONE; `Phase 8 Canvas —
node workflow (8b)` row is **replaced** by 8c–8f.

---

## 10. V1 Scope (Ruthlessly Realistic)

**In:**
- Roles + ModelBindings + Workflows/Runs/NodeRuns data model + JSON (8c).
- Pure DAG scheduler (topo sort, wavefronts, fan-in conjunction, cycle refusal).
- Executor: stateless nodes via `inference::run_prompt`; agentic nodes via the
  opencode session multiplexer (reuse `console`); hardware reservation against
  `hardware_profiles`/`fit`.
- Per-node/run recording, error/timeout/retry, global budget (tokens+wall),
  downward cancellation.
- `CANVAS` view: pan/zoom, node drag, edge create/delete, node config (Role +
  Binding), save/load, run/stop, live per-node status, run history + detail.
- 3 seed templates: Coding Review (linear), Multi-Model Debate (fan-out→judge),
  Research (fan-out→synthesize) — all **DAG, no conditionals/loops**.
- `deck workflow` CLI + Tauri `workflow_*` (executor works headless first).
- `role_id` dimension on `matrix_runs` from day one (cheap, additive).

**Explicitly out (V1.5+):** condition edges/Router, loop node, supervisor/
subgraph, replay, streaming across edges, undo/redo, minimap, multi-select,
alignment, reactflow, per-role *intelligence* (only data capture + a raw per-role
aggregate query in V1).

**Out (V2+):** arbitrary cyclic graphs beyond the loop construct, port-level
fan-in transforms, node templates marketplace, live multi-user.

---

## 11. Future Expansion

- **Arbitrary graphs:** 8f adds conditional edges + loop construct on the
  serialized `Edge.condition`/`Loop` slots already reserved in V1 — no model
  change.
- **Multi-agent workflows:** agentic nodes already run real opencode sessions;
  Roles can nest into subgraphs (supervisor) later without new primitives.
- **Model comparisons in role:** 8e runs a workflow across a binding matrix;
  the existing `compare`/`matrix` report shapes render per-role (agentic runs
  included via captured metrics).
- **Automated model selection:** `recommend` with a role filter turns the raw
  per-role aggregate into "for *Architecture Reviewer*, use Nemotron (94% success)."
- **Agent specialization:** a Role is a stable identity; over time
  `node_runs`/`evaluations` group by (role, model) to reveal specialization.
- **Benchmark-driven role assignment:** feeds → `analyze_relevance` → "offer as
  candidate for vacant Role" (reuses O2/O4/Phase 6 paths).
- **Workflow optimization:** per-node latency/success in `node_runs` lets the
  tool point out the slow/failing node and suggest a different Role/model —
  the sequential-extension of `recommend`.

---

## 12. Risks / Architectural Traps

1. **Coupling the UI directly to execution.** If the canvas *component* owns the
   executor (spawns processes, maps edges to JS), the graph logic is untestable
   and the CLI door can't run workflows. **Mitigation (locked in):** pure DAG
   scheduler + executor in `deck-core`/`deck-engines`; UI only calls
   `workflow_run` and consumes `wf-*` events — identical to how `dl`/`br` work.
2. **Hardcoding models into nodes.** Destroying the Role/Model separation is the
   one decision that would force a rewrite. **Mitigation:** `role_id` is required
   on every node; a Binding is a first-class object; the Workflow doc stores
   `role_id + binding`, never an inline model path.
3. **Treating workflows as simple linear pipelines.** If V1 models only
   `A→B→C`, the scheduler and message model become linear assumptions that are
   hard to undo. **Mitigation:** the V1 scheduler is a real DAG scheduler (fan-in
   gate, wavefronts) even though the seed templates are linear; the Message type
   is port-aware from the start.
4. **Non-reproducible saved layouts.** If a saved workflow stores only "model
   name" with no version/quant and no resolved-deployment snapshot, it silently
   drifts. **Mitigation:** `model_ref` is `family@quant` + `workflow_runs.
   graph_snapshot_json` pins the exact deployment; versioned workflow docs.
5. **Duplicating the benchmark/model systems.** Building a parallel bench DB
   inside the canvas would fork the truth and decay both. **Mitigation:** the
   canvas *writes* `matrix_runs`/`evaluations`/`hardware_profiles` via `role_id`;
   it owns only graph/run/nodes records.
6. **Ignoring GPU VRAM constraints.** A graph that parallel-launches 3 models
   that can't co-fit on the 5070 Ti's 16 GB is an OOM trap. **Mitigation:**
   reservation table vs `hardware_profiles` + `fit.rs`; serialize overloaded
   waves; surface warnings early.
7. **Poor loop handling.** Unbounded loops = runaway token spend. **Mitigation:**
   loops are an explicit construct (never raw cycles), and the V1 budget
   (`max_iterations`, `budget_tokens`, `budget_wall_s`, manual stop) is built
   before any loop exists.
8. **Making future multi-agent execution impossible.** If the executor only ever
   did one-shot prompts, adding tool-using agents later would be a rewrite. 
   **Mitigation:** `Node.kind ∈ {stateless, agentic}` from V1, both routed to
   existing primitives; concurrency + cancellation designed around the session
   model both support.
9. **Adding reactflow/heavy graph deps prematurely.** A heavyweight dependency
   before the domain exists invites coupling. **Mitigation:** hand-rolled V1
   canvas; reactflow only at the 8f polish boundary, as a renderer swap over the
   same serialized doc.
10. **No migration guard.** Adding tables without `schema_version` risks
    unversioned drift across the fleet. **Mitigation:** Phase 0 `schema_version`
    lands *before* 8c; every new table/column is additive + `ensure_column`.

---

## 13. Final Recommendation

**Approach:** Build the Infinite Agent Canvas **domain-first and data-driven,
with Roles separated from Models as the immutable core decision.** The visual
canvas is the last (and easiest) piece, because everything it shows is already a
serializable document executed by a tested, headless engine that both the CLI
and Tauri doors share.

**The single most important commitment:** every node is a **Role** bound via a
**ModelBinding**, not "a model." This one decision is what makes the feature
survive its own growth — it's what turns a canvas into a per-role benchmarking
instrument instead of a pretty wiring diagram.

**Next concrete development milestone — 8c, Workflow Foundation (not the canvas):**
1. Add `role_id` (additive) to `matrix_runs`; land `schema_version` (Phase 0).
2. Build `deck-core::workflow`: Role, ModelBinding, Node, Edge, Workflow, Run,
   NodeRun, JSON + store fns + pure DAG scheduler (topo sort, fan-in gate,
   cycle refusal).
3. Build `deck-engines::workflow`: `workflow_run` executing stateless nodes via
   `run_prompt` and agentic nodes via the opencode session multiplexer, with
   reservation, budget, retry, timeout, and downward cancellation.
4. Ship `deck workflow {save,list,run,stop,history}` + Tauri twins + `wf-*`
   events.
5. **DoD:** `deck workflow run` runs a 2-node Role-bound graph against a live
   slot + an opencode session; `node_runs`/`workflow_runs` are recorded and
   viewable; `cargo test` green. Only *after* this is green does `CANVAS` UI
   (8d) get built on top.
