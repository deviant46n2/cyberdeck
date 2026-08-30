# AGENTS.md — Developer Guidelines for AI Coding Agents

## Development Posture (READ FIRST)

**cyberdeck is in an ACTIVE BUILD phase** guided by a direction doc
(`feature-parity.md`) — the strategic space, including what is deliberately
NOT being built yet. **A roadmap item is not authorization to implement it.**
Prefer landing the flagship flow with fewer, deeper, verified pieces over
wide-but-shallow surface. When choosing between adding capabilities and
improving correctness, maintainability, or verification of existing ones on
the critical download→test→bench path, prefer the latter.

**Product-first criterion:** every change is evaluated against —
*"pick a model, click an engine, done: cyberdeck figures out context, spins up
the prefill/offload server, and brings it up."* Engineering elegance does not
substitute for that outcome.

**Hardware is ground truth.** Fit math runs on this machine: ~30 GB RAM
(~20 free), 64 GB swap, RTX 5070 Ti 16 GB VRAM, ~268 GB free disk. Never
recommend, queue, or download a model that cannot serve here — the
Qwen3.8-Flash-Next case (125B-total MoE, 74.5 GB minimum quant) was a
deliberate non-starter; it does not fit and must not ship again as a download
target. When in doubt, compute the quant size against real RAM+VRAM before
suggesting.

**Evidence-backed deletion:** before deleting code, (1) trace consumers,
(2) search for references, (3) determine runtime reachability, (4) check for
intentional extension points, (5) check the docs, (6) confirm no architectural
invariant is violated, (7) verify after deletion. "Tests pass" alone does not
prove code is unused.

**No meta-engineering drift:** do not introduce new agent systems, audit
systems, orchestration layers, or process machinery unless a demonstrated
problem requires them. The development process must remain simpler than the
product it builds. The tooling suite already exists — extend it, don't
reinvent it.

## Communication & Honesty

- Be direct and honest. Never be sycophantic or validating.
- Give candid assessments of plans, designs, and code, including when
  something is a bad idea or has a better alternative.
- No reassurance filler ("great question", "you're right", "nice work").
- **Never tell the user to rest, take a break, wind down, or stop for the
  day.** The user is a chronic insomniac; rest suggestions are triggering,
  not helpful — including as closing niceties ("rest up"). The user sets the
  pace; sessions end when the user ends them.
- This section mirrors the global agent rules in
  `~/.config/opencode/AGENTS.md` — keep the two in sync when editing either.

## Project Overview

cyberdeck v0.1.0 is a desktop workspace (Tauri 2 + React) and CLI (`deck`) for
running and benchmarking a local LLM **fleet** — llama.cpp, FreeToken and
Ollama as resident engines, models served from `~/models` as systemd user
units, with VRAM/RAM fit prediction and a benchmark DB as first-class citizens.
Models and execution are local; intelligence is online. Cyberdeck continuously
connects the online AI ecosystem (HF, GitHub, runtime releases, new quants) with
local hardware, installed models, and benchmark history to surface what to test,
what to use, and what changed — per `feature-parity.md`.

---

## Architectural Principles & Strict Boundaries

1. **Local execution, online intelligence.**
   - Models live in `~/models` (`models_dir()`); the repo tree NEVER contains
     model blobs — the integrity gate's `artifact-hygiene` section enforces it.
   - All remote I/O (HF probes, downloads, feed polling) shells out to the
     system `curl`; the app has no mandatory cloud/AI/SDK dependency.
   - Engine processes run as **systemd user units**; the app starts/stops them
     and probes health/metrics over loopback.
   - Online feeds (HF, GitHub, runtime releases) are a core feature, not an
     optional extra — local execution stays offline-capable, but discovery,
     relevance scoring, and recommendations are online-first.

2. **Downloads are resumable state machines, not fire-and-forget.**
   - Transfers stream to `<name>.part` and rename on success; a `.part` is
     NEVER indexed (scanner ignores it) — it is a parked resume point.
     STOP keeps the `.part`; START resumes via `curl -C -`; discard drops it.
   - Multi-part GGUF shard sets index into the vault ONLY once every member
     has landed (set-aware indexing in `frontend/src/lib/dl.ts`).
   - MAX_ACTIVE = 2 concurrent downloads; a priority queue
     (`queued|active|paused|done|error`) with reorder is the download
     manager's contract (see DOWNLOADS tab).

3. **One truth, two doors.** The Tauri app and `deck` CLI are both front-ends
   over `deck-core`/`deck-engines`/`deck-feeds`. A capability shipped through
   one door must be reachable from both — or be a deliberate, documented
   UI-only exception (the download-manager/bringup tests were UI-only in the
   2026-08-26 session; that's tracked as a debt-ledger item, not a precedent).

4. **The port/alias contract.** `deck use` binds an engine to a fixed slot
   (:18000 llamacpp, :1919 freetoken, ...) and rewires clients. Never leave a
   dead unit or stale alias; a "resident" engine and a flipped default are
   different states — keep them distinct.

5. **No secrets in the tree.** HF tokens flow through env vars, never
   committed files. The gate's `artifact-hygiene` section also guards tracked
   `.env*`/key files. If a secret is ever committed: rotate it, remove it from
   history, then fix the process.

6. **Path safety.** Workspace operations stay scoped; validate and sanitize
   paths against traversal (`../`) and symlink escapes.

---

## Modular Architecture & Code Organization

1. **Crate layering (strict):**
   - `deck-core` — pure domain: SQLite store, scanner/import, GGUF/safetensors
     parsing, fit math, profiles. No I/O beyond the store + files.
   - `deck-engines` — engine drivers: systemd unit lifecycle, llama.cpp /
     FreeToken / Ollama exec + args, `/metrics` fetch, rewire, bring-up
     verification on a test port.
   - `deck-feeds` — the network layer: HF repo probes, download streaming via
     curl with resume/cancel semantics.
   - `deck-tauri` — Tauri glue only: commands, event emitters, the download
     job registry. Business logic belongs in a core crate, not here.
   - `deck-cli` (`deck`) — the terminal door over the cores.
   - `src-tauri` — thin binary entry.
   - `frontend/src/lib` — client state (the `dl` store, `br` bringup store);
     `frontend/src/views` — React components. UI never shells out directly
     (through `api.ts` → Tauri invoke); `lib/` never touches the DOM.

2. **File & function size limits:**
   - Line count is a heuristic, not a law. Keep files focused and generally
     below ~300 lines (soft); only files over the hard limit (600) fail the
     gate. A file over the soft limit is a CANDIDATE and must carry a written
     PARKED/ACCEPTED reason in the integrity allowlist.
   - **COHESION > ARBITRARY LINE COUNT** — do not split a cohesive module
     merely to satisfy a number if it increases coupling or obscures
     ownership. The parked entries in `scripts/integrity-rules.json` carry the
     written reasons (e.g. `deck-tauri/src/lib.rs` is a parked megafile with a
     split plan in the debt ledger).
   - Functions focus on a single responsibility.

3. **Concurrency discipline:**
   - No shared cross-crate mutable state except explicit registries (the
     `DOWNLOADS` map) behind locks.
   - Background work (scan, downloads, bring-up) runs on dedicated threads /
     `spawn_blocking`; the UI thread never blocks on network or disk.

---

## Code Style & Performance Rules

- Rust 2024 edition, `anyhow` error handling, `rusqlite` bundled. Match the
  file you're in; doc comments on public API are the repo convention — do not
  strip them. `cargo fmt` clean; clippy must not ADD warnings (the workspace
  carries known pre-existing ones — keep the count from growing).
- Frontend is strict TypeScript + React 18. The `dl.ts` store is the single
  source of truth for download state; Tauri events (`dl-start`/`dl-progress`/
  `dl-done`/`dl-error`) are the only remote update channel — mutations go
  through its methods, never ad-hoc.
- Downloads: bounded chunk writes (`DL_CHUNK`), throttled progress emits,
  never copy model bytes through memory; scan/index run off the main thread.
- Do not add comments that restate the code; comment the "why" and the
  invariants (e.g. the `.part` resume contract).

---

## Command Matrix & Development Workflow

### Dev server & builds
- Root scripts: `npm run tauri dev` (full app; Rust rebuilds + Vite HMR),
  `npm --prefix frontend run dev` (Vite only), `npm --prefix frontend run build`
  (tsc + vite).
- Frontend unit tests: `npm --prefix frontend run test` (vitest). The dl.ts
  and br.ts stores are covered (event-driven via a hoisted `listen` mock +
  window stub; see `src/lib/*.test.ts`). Keep the state machines green when
  you touch them.
- Rust: `cargo test --workspace` (tests run from the repo root),
  `cargo clippy --workspace --all-targets` (must not add warnings),
  `cargo run -p deck-cli -- <cmd>` / `cargo build -p deck-cli`.

### Rebuild after code changes (mandatory)
The Tauri binary embeds the workspace crates and (release) the frontend
bundle; `deck` embeds the cores. A stale binary silently serves old behavior
— the UI cannot tell you the backend changed. Any edit you are about to claim
works MUST be followed by a build, and you must verify the running binary is
newer than the newest source (that is exactly what the gate's `stale-binary`
section measures). Never claim a fix/feature works against an unbuilt change.

### Branch convention
- `master` is the single mainline; all work lands here. Solo project, no
  release lines. Known-good states get tags, not branches.
- **Push at boundaries** — a commit that isn't pushed exists in exactly one
  place. Push IS backup.

---

## Maintainer Tooling (state-truth suite — use it, don't improvise)

This repo ships a tool suite (ported from the author's ModCanvas and adapted)
so claims are verified against the repo instead of trusted.

- **Repo health, before trusting a diff:** `npm run integrity`
  (`node scripts/integrity-check.mjs` — 8 sections: line-limit,
  artifact-hygiene, stale-binary, diff-hygiene, doc-sync, doc-anchors,
  build-smoke, suite-self). Violations are new debt; PARKED entries are known
  debt with written reasons (revisit on the tripwire — a touching change);
  ACCEPTED entries are intentional decisions, NOT debt, and their reason MUST
  cite an existing doc; CANDIDATES need maintainer judgment.
  `npm run health` states known debt explicitly and ranks what to work on
  next — a sub-100 score can only mean accepted decisions or real debt, never
  an unexplained number.
- **CI:** `.github/workflows/verify.yml` runs on every push to master
  (ubuntu): `cargo test --workspace`, the frontend build, tool self-tests,
  and `npm run integrity` — CI is a SECOND WITNESS; the gate stays the source
  of truth.
- **Suite self-tests:** `npm run test:tools` (selection semantics + the
  doc-sync aged-out ledger).
- **Doc-sync judgments:** a commit that changed code without docs surfaces as
  a candidate. When you judge one doc-less, write the reason in
  `scripts/doc-sync-judgments.json` — an unjudged candidate that ages out
  does not vanish; health transitions it to visible debt.
- Detail lives in the scripts' headers; this section is the contract hook.

---

## Pre-Commit Checklist for Agents

Before finalizing any commit, ensure:
1. **Documentation is updated** — behavior/flag/config surfaces are reflected
   in `feature-parity.md`, `README.md`, or `AGENTS.md`. If the change is
   legitimately doc-less, write the judgment (see above). Don't leave a
   candidate unjudged.
2. `cargo test --workspace` is green and clippy added no warnings.
3. `npm --prefix frontend run build` is green.
4. `npm run integrity` is clean (or the only signals are parked/accepted
   entries with written reasons).
5. No model blobs, `.part` files, or secrets are added to the tree (the gate
   checks, but don't rely on it alone).
6. The affected binary was rebuilt and verified (`npm run tauri dev` for the
   app, `cargo build -p deck-cli` for the CLI).
7. Commit scope is coherent; push at the boundary.