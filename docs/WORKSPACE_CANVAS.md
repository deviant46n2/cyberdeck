# WORKSPACE Canvas — Single Centerpiece (HUD + Loadouts + Canvas unified)

Replaces `HUD`, `LOADOUTS`, `CANVAS` nav entries with one `WORKSPACE` view. Goal: multi-agent loop is end-to-end benchmarkable.

## Why

- HUD ctx slider was disconnected from loadout ctx (loadout wins). Users had to switch views to understand why `ctx 131072` still `exceeds 14336`.
- HUD sessions orphaned when navigating away (fixed in Hud.tsx: `runningSessionIds` + `onUnmount opencodeStop`).
- Roadmap 8f/8e (branch/loop, per-role bench) needs a single spatial surface to visualize loops and compare `tok/s` per role and whole-loop aggregate.

Recon synthesis (CrewAI/LangGraph/TermCanvas/Widescope):
- **LangGraph** for durable DAG + conditional/loop edges + checkpoints → already in `WorkflowEdge{condition, loop_edge}` and `matrix_runs`.
- **CrewAI** for role ergonomics (researcher/writer/reviewer → `role_id`) + drag-to-bind loadout.
- **TermCanvas** for infinite canvas pan/zoom spatial memory (single coordinate space for agents + workflow nodes).
- **Widescope** for loop-iteration grouping in trace view.

## Layout

```
+---------------------------------------------------------------------+
| WORKSPACE header: workflow picker + runner + unified model picker   |
| [workflow ▼] [stateless|agentic] [dir if agentic] [▶ RUN][■ STOP]  |
| unified model: local GGUF 🔵 / cloud 🟣 (openrouter/anthropic)     |
+---------------------------------------------------------------------+
| left 260px            | infinite canvas (pan/zoom, SVG edges)       | right drawer |
| WORKFLOWS list        |  nodes share coords, colors differentiate:  | inspector    |
| + LOADOUT palette     |   Agentic=magenta, Stateless=cyan,          | simplified:  |
| drag → bind           |   TUI=panel-2, Loadout=magenta border      | name/engine/ |
| PER-ROLE BENCH        |  ctx slider ONLY if isLocal (cloud=mute)   | model/port/  |
| RUN HISTORY           |  status border: pass/warn/oom               | ctx + fit    |
|                       |  edges: solid / magenta loop / yellow cond | [Advanced ▸] |
+-----------------------+---------------------------------------------+--------------+
| bottom: chat bar (prompt + auto-approve + dir) — prompt spawns      |
| new Agent node on canvas at cascade offset; dir from header          |
+---------------------------------------------------------------------+
| footer: WHOLE-LOOP BENCH (aggregate) + PER-ROLE BENCH table          |
| whole-loop: total gen_tokens / wall_ms → loop tok/s, iterations      |
+---------------------------------------------------------------------+
```

- `VIEWS = ["WORKSPACE","VAULT","SIGNALS","FEEDS","MARKET","DOWNLOADS","BENCH","COMPARE"]` in `App.tsx`. Legacy routes kept behind `?legacy=1` for debug.
- Left palette is 240–260px, drawer is 360px (collapsible), canvas flexes. Chat bar sticky bottom, bench footer collapsible.

## Node Types (everything is a node)

| Kind | Source | Configurable fields | Ctx | Engine |
|------|--------|---------------------|-----|--------|
| `Agent` | HUD session (`sessions[]`) | prompt, model_ref, per-session ctx, running/log | slider if isLocal | unified picker |
| `WorkflowNode` | Canvas DAG (`WorkflowNode`) | role_id, binding{model_ref, engine, overrides_json}, exec{timeout/max_tokens}, pos | per-node ctx (local only) | binding.engine |
| `Loadout` | Profile (`ProfileRow/Profile`) | name, engine, bin, alias, port, host, ctx_size + advanced (n_gpu_layers, kv_*, etc.) | ctx_size | LlamaCpp/FreeToken |
| `TUI` | `tui_spawn` pane | dir, cols/rows, pos, size, zIndex | n/a | n/a |
| `Workflow` (container) | `Workflow` | name, description, nodes[], edges[], exec_settings | global max_iterations | n/a |

Cloud models: `isLocal = false` when `model_ref` starts with `openrouter/`/`anthropic/`/`ollama/`-cloud. Badge 🟣, ctx slider disabled + label `ctx mute`.

## Data Flow

```
App.tsx: models, profiles, onChanged(refresh) → Workspace
  header: workflowSeed/list, workflowHistory, engineStatus, benchHistory → residents/benchBySlot
  canvas: listen wf-node/wf-done + opencode-started/output/done → sessions + status Map + runningSessionIds
  left: workflowList + profiles (drag)
  drawer: profileGet → LoadoutEditor (simplified→advanced), saveProfile/deleteProfile → onChanged
  bottom: prompt → opencodeRun({prompt,dir,auto,model,engine,ctx}) or workflowRun
  footer: workflowPerRoleBench + new workflowLoopBench (aggregate)
```

- Single `runningSessionIds` ref tracks both HUD agents and TUI panes; unified `onUnmount` stops all (HUD orphan fix).
- Loadout drag → drop on Agent/WorkflowNode: sets `node.binding.model_ref = profile.model`, `binding.engine = profile.engine`, `overrides_json` carries ctx delta; does **not** auto-APPLY (no restart). Badge `↻ restart required` if loadout `ctx_size` diverges from `port_map_status` resident.
- `PREVIEW` (dry-run `renderProfileUnit`) vs `APPLY` (`useProfile` restart) stays in drawer.

## Bench: Whole-Loop

New query `workflow_loop_bench(workflow_id)` → `Vec<LoopBenchRow{workflow_id, runs, best_loop_tps, avg_loop_tps, last_tps, last_wall, last_iterations, last_tokens}>`
- from `matrix_runs` JOIN `workflow_runs`: per run, `loop_tps = sum(gen_tokens)/max(wall_ms)` grouped by workflow_id+run_id, then aggregate across runs.
- Footer shows one row per workflow (the selected one expanded) + per-role table below. Makes supervisor loops comparable.

## File Map

- `frontend/src/views/Workspace.tsx` — new shell, ~400 lines (soft limit, cohesive). Imports Hud/Canvas/Loadout logic as subcomponents initially.
- `frontend/src/lib/workspace.ts` — pure helpers (isLocalModel, ctxForNode, loopTps) + tests.
- `crates/deck-core/src/store.rs` — add `loop_bench()` query.
- `crates/deck-tauri/src/lib.rs` + `src-tauri/src/main.rs` — new Tauri cmd `workflow_loop_bench`.
- `frontend/src/api.ts` — `workflowLoopBench()` + `LoopBenchRow`.

## Incremental Plan

1. Shell + route merge (no behavior change) — this PR.
2. Unify canvas coords (HUD sessions + workflow nodes + TUI panes share `cardPos`/`pos` map, one SVG layer).
3. Whole-loop bench backend + footer row.
4. Drawer polish + drag-to-bind.

## Invariants

- Local execution, online intelligence unchanged.
- `.part` resume, systemd units, port contract unchanged.
- One truth: `DownloadManager`/`profile` remain source of truth for both doors.
