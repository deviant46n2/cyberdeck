# Cyberdeck — Future Ideas Backlog

Ideas for Cyberdeck that are **not** active roadmap work. Nothing here is a
commitment. The standing priority is unchanged: **get the Core Loop MVP
functional and use Cyberdeck as a daily tool before expanding the feature set.**

Read `ROADMAP.md` first — many of these ideas already have a home there
(cross-referenced below). This file only holds what the roadmap does *not* yet
capture, plus the post-MVP depth of partially-covered ideas.

---

## Already covered by ROADMAP.md (no new work here)

| Idea | Where it lives |
|------|----------------|
| Reproducible experiments / provenance | `ROADMAP.md` Phase 1 — **staged provenance** (`model identity/rev/quant`, `engine/version`, `hardware profile`, `workload`, `ctx`, `sampling`, `prompt/gen tokens`, `prompt_tps`/`tok_s`/`TTFT`/`wall_ms`, `peak VRAM`, `verdict`, `evaluator/version`, `measurement method`, `workflow/run id`; guiding principle: never show a number without being able to explain what produced it) + Phase 3 (`hardware_profiles` + `engine_version`) |
| Hardware-aware feasibility | Already exists: `fit.rs` estimate + `derive_loadout` + BringUp test-port verify; refined by Phase 1 `peak_vram_mb` + Phase 3 VERIFIED/MEASURED tiers |
| Experiment failure classification | Phase 6 "Failure is data" (`verdict=CRASH/OOM`, `⚠ CRASH` not silent zero) |
| Agent sandboxing / permissions | Phase 7 permission ladder (READ/ANALYZE/MODIFY/EXECUTE/AUTONOMOUS) + O3 audit log (now hardening toward typed semantic settings: `default engine/model`, `ctx reserve`, `download concurrency`, `auto-bench`, `agent permission`, `resource limits`) |
| Process lifecycle / crash safety | `ROADMAP.md` Phase 0R — every `Child` has an explicit owner, `opencode_stop` vs waiter race closed, `kill_all` reaps, `PDEATHSIG` + process-group + `console_reaper.rs` `AGENT_MARKER` preserved; regression gate (normal/stop/group/shutdown/crash) |
| Host portability | `ROADMAP.md` Phase 0P — no `/home/deviant` literals, no silent `ollama/qwen3.8:27b` fallback, test taxonomy pure/host/hw (fresh clone never inherits dev machine) |
| Evaluation dimensions | `ROADMAP.md` Phase 2 — five separate dimensions (`performance`, `quality`, `task success`, `reliability/failure`, `resource`) never collapsed to one opaque score; deterministic + human evaluators coexist; `EchoRunner`/`Human` remain as deterministic infra |
| Recommendation as evidence pipeline | `ROADMAP.md` Phase 4 — downstream `DISCOVER→FIT→RUN→MEASURE→EVALUATE→COMPARE→RECOMMEND` over actual bench/workload/hardware/quality data, explainable rank not opaque "AI recommends" |
| Core experimental loop gate | `ROADMAP.md` Strategic guardrail + MVP guardrail — `Select workload → Select candidates → Execute → Measure → Evaluate → Compare → Explain → Recommend`; no new major UI surface until this loop is trustworthy |
| Documentation alignment | `ROADMAP.md` O6 — `README`/`ROADMAP`/`FUTURE`/`docs/WORKSPACE_CANVAS.md` aligned to WORKSPACE (`App.tsx VIEWS` + `?legacy=1`) and current/legacy/active/future distinction |
| Event/streaming of runs | Landed: `console.rs` opencode streaming + `wf-*` events (Phase 8d) |
| Real-world agent benchmarking | Phase 2 repo-local tasks (`pytest`, `cargo test`, `patch_apply`) + Phase 8 workflows + per-role bench (8e) |
| Blind model trials | **Already implemented** — `deck bench compare` hides candidates as `trial-NNN` |
| Regression detection | Phase 9 / O10 self-optimization (perf delta → inspect → revert) |
| Background / continuous benchmarking | Phase 9 (opt-in overnight) + O5 background polling service |
| Deliberate non-goals | `ROADMAP.md` "Things Not To Build Yet (Explicit)" |

---

## Architectural invariants to preserve now (no implementation)

These are cheap habits that keep future options open without building anything.
They are also written as a short note in `ROADMAP.md`.

1. **Results are immutable + self-describing.** `matrix_runs` / `node_runs` are
   INSERT-only history. Never mutate or reinterpret a stored result under the
   *current* workflow/model/engine config — snapshot the config (or a version
   id) inside the row at run time so "workflow v7 result" always means v7.
   This is what the roadmap's additive-schema plan already implies; keep it a
   rule, not an accident.

2. **Agent is an abstraction seam, not a hardcode.** Workflow/experiment
   internals talk to a runner interface (`AgenticRunner` / `StatelessRunner`),
   never to `opencode` by name. Record agent identity + version as run
   provenance so the harness becomes a benchmarkable variable later.

3. **Telemetry rides the result rows.** Keep perf fields (`tok_s`, `ttft_ms`,
   `gen_tokens`, `peak_vram`) on `node_runs`/`matrix_runs` even when the UI
   doesn't show them yet — the Tamagotchi, observability, and observability
   dashboards are all downstream consumers.

---

## FUTURE — post-MVP ideas (not on any roadmap phase)

### Hardware Tamagotchi
A small persistent hardware mascot reflecting GPU util / VRAM / temp /
experiments run / models tested / OOM history / milestones. Pure
visualization-personality layer — explicitly **no** process control or
automated decisions. First version must be small; it's a consumer of the
telemetry invariant above, not a driver of it.

### Model lifecycle management
Distinguish `discovered → available → downloaded → installed → tested →
preferred → obsolete → archived`, and know which exact model/quant is installed
and which workflows depend on it. Scanner/vault + `matrix_runs` already feed
partial signals; build states on top of discovery/management work, not as a
separate system.

### Model storage management
Usage reporting, duplicate detection (a `Dedup.tsx` view exists), unused-model
detection, dependency-checked deletion, multi-location storage. Any deletion
must consult workflow references (`node_runs`) — same rule as the roadmap's
high-risk-permission gate.

### Artifact system
Agent nodes produce structured artifacts (git diffs, modified files, logs,
test results, screenshots) consumed by downstream nodes instead of giant text
prompts. Requires the node-result append-only habit; this is a layer, not a
prerequisite.

### Resource & job management
Queued/concurrent experiments, limits, cancellation, timeouts, pause/resume,
GPU allocation, priority. Precedent to copy: the `DownloadManager` priority
queue in `deck-feeds`. Support autonomous runs later without an MVP dependency.

### Agent runtime abstraction
Treat coding agents/harnesses as interchangeable experiment components —
OpenCode, Goose, OpenHands, future frameworks — behind one common
agent/runtime interface, so the agent itself becomes a benchmarkable variable
(`Model × Agent × Backend × Workflow`). The runner seam is already the
architectural habit; the *multi-provider* surface is the future work.

### Workflow / experiment question engine
"Find the best local coding setup for this repository" → Cyberdeck decides the
experiment set to answer it. Foundation for autonomous experiment design.

### Compute-budget-aware experimentation
Progressive funnel: candidates → hardware feasibility → cheap screening →
promising → expensive quality tests → workflow tests → finalists. Spend compute
where information is maximized.

### Self-configuring workflows *(explicitly not MVP — per the idea itself)*
An agent constructs workflows through a *constrained* workflow API
(`create_node/connect/set_model/run/inspect_result/compare`), never application
internals. Major long-term vision.

### Workflow evolution / optimization
Generate alternative workflow variants (swap model/agent/prompt, reorder nodes),
evaluate, keep winners. Only once the basic workflow/experiment system is mature.

### Personal objective functions
"Best" becomes workload-personalized (correctness vs speed vs VRAM vs
reliability vs tool success). Phase 4's weighting presets are the seed; learning
what a user actually optimizes for is the future.

### Evidence-based recommendations with confidence
Explainable recommendations plus confidence (low/moderate/high) from
evidence volume/quality. Phase 4 gives the sentence today.

### Personal model/agent knowledge
Accumulated empirical knowledge of what works *on this machine* (models for
coding/review, agent×model fits, backend×quant fits, recurring failures) — from
Cyberdeck's own runs, not generic rankings.

### Human override / feedback capture
Record recommendation → user choice → reason → outcome, and let the objective
function absorb the mismatch. Audit log is the precedent.

### Workflow/model obsolescence detection
Re-test saved workflows as the ecosystem moves: "not tested in 6 months; three
newer models beat its components — re-test?" Consumes feeds scoring + bench
history.

### Import / export of experiments and workflows
Serialize graph, node config, prompts, model/agent identifiers, deps, bench
config between installs. Foundation already exists (`deck workflow save --file`).
No model blobs in this format.

---

*This backlog is deliberately selective: most "roadmap-shaped" ideas already
live in `ROADMAP.md`. If an idea proves valuable during core-loop use, promote
it to a roadmap phase then — not before.*

---

## 2026-08-31 audit — what moved to ROADMAP.md

The audit that produced `Phase 0R` (process ownership & reaping), `Phase 0P` (host portability & test taxonomy), the staged provenance principle in `Phase 1`, the five-dimension evaluation invariant in `Phase 2`, the `DISCOVER→…→RECOMMEND` downstream placement in `Phase 4`, the typed semantic settings hardening in `O3`, and `O6` docs synchronization was a **planning-only** pass: no source code was changed. `FUTURE.md` itself was not expanded with new speculative items — the audit findings were deliberately placed on the active roadmap so they gate the core experimental loop, rather than being parked as future ideas.