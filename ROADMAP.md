# Cyberdeck Roadmap — Full Analysis (2026-08-30)

> **Product principle:** *Cyberdeck continuously connects the rapidly changing online AI ecosystem with the user's actual local hardware, models, runtimes, workloads, and benchmark history, then uses that accumulated evidence to discover, test, configure, select, and operate the best local AI models.* Models/runs are local; intelligence is online; the agent becomes the operator.

Previous correction (2026-08-29) already removed the offline-first constraint from `AGENTS.md`/`feature-parity.md`/`README.md` and introduced Horizons O1–O3. This document completes the analysis with Workload, Evaluation, Hardware, and Agent depth and maps every recommendation to the actual codebase.

---

## Executive Summary

Cyberdeck's existing strength is **fleet control + fit + bringup + isolated bench** — systemd units per engine-fixed port (`:18000`/`:1919`/`:11434`), GGUF/safetensors parsing (`deck-core`), VRAM fit (`fit.rs` → PASS/WARN/OOM), derived max-ctx `BringUp` (`deck-engines::derive_loadout` → test port `18999`/`18998` → bench → apply), and the matrix/compare grid (`matrix_runs` + `bench` tables, `health::measure_generation_tps` as the single measurement path). That's the differentiator over Odysseus and it must not be rewritten.

What's missing for the stated vision is **what to bench and why**: no Workload concept, evaluation is a lexical placeholder (`scoring.rs`: `0.25·variety + 0.75·(1-bigram)`), hardware is ephemeral (`nvidia-smi` per call, not a persistent profile), raw bench provenance is thin, online discovery is now founded (O1 `releases` table + `deck-feeds::feeds` adapters + `deck feeds poll/list`) but no relevance scoring / personalized MARKET, no recommendation engine, no typed settings/audit, and the agent is still opencode-passthrough rather than a Cyberdeck operator.

The roadmap below keeps the **one-truth-two-doors** contract (`deck-core`/`deck-engines`/`deck-feeds` → `deck-cli` + `deck-tauri` → Tauri/React), evolves dimensions incrementally, and lands the smallest loop that can truthfully answer *"what should I use for this workload on this hardware, and why?"*

---

## Current State Audit

**Deck-core** — `store.rs` (770 lines, accepted debt): `models`, `profiles`, `meta`, `residents` (PORT MAP), `bench`, `matrix_runs`, `engine_bin`, `releases` (NEW O1), plus `watchlist`/`seen` via `deck-feeds::watchlist` on same DB. `gguf.rs` (699 lines, parked) EOF-tolerant Range parse; `safetensors.rs` (accepted); `fit.rs` pure `estimate(ModelMeta, FitRequest, available_mb) → FitBreakdown` with GQA-correct `n_head/n_head_kv`, VRAM + KV split; `scanner.rs` → `ModelMeta`; `profile.rs` (accepted) `Engine`→`EngineDescriptor` registry + `derive_loadout` per engine; `dedup.rs`; `importer.rs`.

*Exists:* fit engine (B1 DONE), resident PORT MAP (DONE + `PortMap.tsx`), BringUp derive+verify.

*Partial:* `store.rs` has no migration/version table (schema is `CREATE IF NOT EXISTS` only); `Release` now exists but `Settings`/`AuditLog`/`HardwareProfile`/`Workload` missing; `matrix_runs`/`bench` lack hardware/workload/provenance linkage.

**Deck-engines** — `systemd.rs`/`unit.rs`→`render_unit` + `.bak`, `inference.rs` protocol per `EngineDescriptor` (llama.cpp `/v1/completions`, Ollama `/api/chat`), `health.rs` (accepted) single measurement path `measure_generation_tps` + `boot_on_test_port`, `grid.rs` shared cell builder, `matrix.rs` sequential isolated cells (VRAM-safe) recording raw ingredients → `matrix_runs`, `compare.rs` blind `trial-NNN` scoring over same grid, `scoring.rs` placeholder lexical quality + normalized `tok_s`, `status.rs`, `rewire.rs`, `lib.rs`.

*Partial:* `matrix.rs` already grid-is `model × quant × engine`; no `configuration × workload × hardware` dimensions, no TTFT/prompt-TPS/VRAM telemetry, scoring only lexical.

**Deck-feeds** — `probe.rs`/`market.rs`/`download.rs`(accepted)/`watchlist.rs`/`ollama.rs`/`feeds.rs` (NEW, 269 lines). `feeds.rs` adds `Source` trait + `HfSource` (watchlist orgs → `hf` releases, `sha|lastModified|createdAt` as rev) + `GithubSource` (`github` releases per repo, `tag_name` as rev, per-repo skip on 404, `GITHUB_TOKEN` auth) + `poll([hf|github]) → (fetched, inserted)`. CLI `deck feeds poll/list` and Tauri `feeds_poll/feeds_list` land with 54 workspace tests green.

*Partial:* Adapters are extensible but O2 relevance scoring, caching/ETag, intervals, notifications not yet.

**Deck-cli / Deck-tauri / Frontend** — Two doors over same cores (never shell out directly; `frontend/src/api.ts` accepted 305 lines is the single invoke door). Views: `Hud.tsx`/`PortMap.tsx`/`Vault.tsx`/`Market.tsx`(accepted 529)/`Loadouts.tsx`/`LoadoutEditor.tsx`/`Bench.tsx`/`Bringup.tsx`(accepted)/`Console.tsx`/`Signals.tsx`/`Dedup.tsx`/`Downloads.tsx` + stores `dl.ts`(accepted)/`br.ts`. `Console.tsx` runs `opencode run` sessions concurrently.

*Partial:* Signals is still HF-only filtered list; Market has remote FIT + DISK prefetch but not personalized relevance; no Workload picker; no Feeds view yet; no Settings UI.

**Verdict:** Do not rewrite `fit`/`gguf`/`matrix`/`health`. Extend them.

---

## Target Architecture

```
                INTERNET
                   │
      ┌────────────┼─────────────┐
      ↓            ↓             ↓
 Hugging Face  GitHub      Other Feeds
      │            │             │
      └────────────┼─────────────┘
                   ↓
            deck-feeds/feeds  ← Source trait, N adapters
                   ↓
         deck-core releases catalog  (dedup source:repo@rev)
                   ↓
        Relevance scoring (hw + workloads + bench history)
                   ↓
              MARKET (personalized: “what to download/test?”)
                   ↓
        deck-core hardware profile  ──┐
                                      ↓
   LOCAL: scanner→models → fit → derive_loadout(BringUp) → systemd units(:18000/:1919/:11434)
                                      │
                               matrix × workloads (isolated cells)
                                      │
                          MEASURE → EVALUATE → AGGREGATE
                                      │
                               bench + matrix_runs (+ raw output, hw rev, engine ver)
                                      │
                  ┌───────────────────┴───────────────────┐
                  ↓                                       ↓
          Recommendation engine (explainable, deterministic)
                  ↓                                       ↓
          UI: “what to use / what to test / what changed / is anything broken”
                  │
               Agent tools (typed deck-* APIs, not shell)
```

**Boundaries preserved:** `deck-core` = pure domain (store, parsers, fit, scoring interfaces, workloads, hardware profile, settings, audit). `deck-engines` = lifecycle + harness. `deck-feeds` = adapters + transport. `deck-tauri` = glue only. Frontend = `api.ts`→invoke, stores never touch DOM.

---

## Domain Model

```rust
Workload { id, label, description, tasks[], evaluator } // tasks = label=prompt templates

Task { id, workload_id, label, prompt_template, dataset_ref? }

HardwareProfile { id, gpu, vram_mb, cpu, ram_mb, os, driver, cuda, cyberdeck_ver, captured_at }

Release { source, repo, rev, kind, title, url, published_at, payload_json, fetched_at }
  // PK (source, repo, rev) — new in O1

BenchContext { hardware_profile_id, engine, engine_version, profile_snapshot, ctx, sampling }

MatrixRun { // existing matrix_runs + columns
  workload_id?, hardware_profile_id?, engine_version?,
  prompt_tps, ttft_ms, peak_vram_mb?, model_rev/hash?, ..existing
}

Evaluation { run_id, method: Deterministic|ModelJudge|Human,
             passed: bool, score: f64, details_json }

Settings { key, value_json, schema_version } + AuditLog { ts, actor, key, old, new, reason }
```

**Decision:** Workload is a first-class table, not a string tag. Everything else (grid, evaluation, recommendation, releases) references it. Add incrementally; existing rows get `workload_id = NULL` (backwards compat).

---

## Benchmark/Evaluation Architecture

**Current:** `scoring.rs` lexical placeholder `score = 0.6·quality(0.25·variety+0.75·(1-bigram)) + 0.4·norm_tok_s`, `health::measure_generation_tps` → `BenchRow`, `MatrixRow` keeps raw `prompt_tokens/gen_tokens/wall_ms/output/tok_s+t ok_s_kind`.

**Target (pluggable, layered — leverage OSS harnesses, don't reimplement):**

```
MEASUREMENT  — wall_ms, prompt_tokens, gen_tokens, tok_s(tok_s_kind), ttft_ms, prompt_tps, peak_vram (when available)
               Adapters: llama-bench / llama.cpp --bench for tok_s, nvidia-smi dmon for peak_vram,
               plus fallback inference.rs probe. Same GenSample struct, different source.
    ↓
EVALUATION   — trait Evaluator { fn evaluate(&self, output: &str, ctx: &EvalCtx) -> Evaluation }
               Deterministic (OSS-leveraged):
                 exact/regex/JSON-schema via harness libs
                 lm-evaluation-harness (Eleuther) → Evaluator::LmEval { tasks, harness_version }
                   coding→{humaneval,mbpp}  reasoning→{gsm8k,mmlu}  instruction→{ifeval}
                   invoked as `lm-eval --model hf --tasks <csv> --output_path /tmp/eval`
                   parsed → Evaluation rows; gated on `which lm-eval`, fallback to native exact/regex
                 compile+test/patch-apply via pytest/cargo test (repo-local)
               ModelJudge: pairwise/rubric via a resident judge model (same harness, separate engine)
               Human: future UI flag
               Native tiny evaluators (exact/regex/json/compile) stay for custom repo tasks;
               standard benchmarks delegate to lm-eval/HELM/OpenCompass rather than re-deriving them.
    ↓
AGGREGATION  — per candidate (model×quant×engine×config×workload) → ok_rate, mean quality, P50/P90 tok_s/ttft
    ↓
RECOMMENDATION — deterministic explainable query over aggregates
```

**Contract:** Never collapse to one opaque score. Store quality + success + tok_s + ttft + failure independent; overall score (if shown) is derived, auditable. Preserve raw `output` and `evaluation details_json` + `evaluator_version`/`engine_version` in store. Keep `tok_s_kind` honesty (`native` vs `wall`). Harness is an evaluator implementation, not a new service — `deck-engines::evaluation` owns the trait, `lm-eval` is one arm.

---

## Hardware Architecture

**Today:** `fit::hw_vram()` live probes `nvidia-smi` per call; `fit::available_vram_mb(fallback)`; no persistence.

**Target:**

* `HardwareProfile` captured at startup/bench time: `gpu (nvidia-smi --query-gpu name, vram)`, `cpu (/proc/cpuinfo)`, `ram (/proc/meminfo)`, `os (os-release)`, `driver (nvidia-smi --query-gpu driver_version)`, `cuda (nvcc/runtime)`, `cyberdeck_ver`, `engines {id, bin, --version}`.
* `store::capture_hardware_profile() -> id` — upsert by content hash; reuse id when unchanged.
* `bench`/`matrix_runs` FK → `hardware_profile_id`.
* Profile versioning: content change → new row (history survives).
* VRAM trust tiers: `ESTIMATED` (fit math) vs `VERIFIED` (test-port boot succeeded) vs `MEASURED` (`/metrics` + `nvidia-smi dmon` peak if available). UI badges show tier; never show estimate as truth when a measurement exists. Future empirical calibration: `error = measured - predicted` logged.

---

## Online Intelligence Architecture

*Already landed O1:* `deck-feeds::feeds::Source` (`id()+fetch()->Vec<Release>`) + `HfSource` + `GithubSource`, `deck_core::store::Release` catalog (PK dedup), `deck feeds poll --source hf|github` / `deck feeds list --json --limit`, `deck-tauri::feeds_{poll,list}`.

*Target:*

* `deck-feeds/feeds/` — add adapters as files (OpenRouter, RSS, quant feeds) without touching poller. Per-source `interval + jitter + ETag/Last-Modified cache` on disk (`~/.local/share/cyberdeck/feeds/{source}/{repo}.json`), 429 backoff, `HF_TOKEN`/`GITHUB_TOKEN` via env. `poll` respects `sources` filter; `list` orders by `fetched_at DESC`.
* `releases` stays raw JSON payload; schema evolves without migrations.
* Dedup by `source:repo@rev`; ignore same rev, new rev inserts + triggers scoring.
* Notifications: Tauri tray/probe or HUD badge for `NEW→RELEVANT→WORTH_TESTING`.
* Do not build a distributed crawler; curl + SQLite is enough.

---

## Agent Architecture

**Today:** `deck-tauri::console` → `opencode run` passthrough, no Cyberdeck tools; permissions are ambient (user runs CLI).

**Target (permission ladder, typed APIs over shell):**

```
READ: hw, models, releases, engines, logs, bench, workloads, recommendations
ANALYZE: relevance, regressions, config issues, experiment selection
MODIFY CYBERDECK: settings/profiles/workloads/monitoring via settings API (validated, audited, revertible)
EXECUTE: download / engine start-stop / bench matrix/compare (explicit consent, costs disk/VRAM)
AUTONOMOUS: scheduled poll + auto-bench (opt-in only)
```

* Agent tools are Tauri/CLI typed verbs (`feeds_poll/list`, `hw_info`, `bench_history`, `matrix/compare`, `use_profile`, `settings get/set`) — not `bash`.
* High-risk (delete models, rewrite units outside `deck`, burn disk/VRAM) requires explicit authorization.
* Settings writes record `(ts, actor=agent, key, old, new, reason)` → `audit_log`, exposed as `deck settings log` + revert.

---

## Recommendation System

**Answers:** Best per workload, fastest useful, best quality/VRAM ratio, best overall for this hardware, new model worth testing.

**Start deterministic:** No ML initially. Query pattern per workload view:

```
candidates(workload) → group by (engine, model, quant, profile)
→ aggregates: success_rate, mean_quality, P50 tok_s/ttft, fit verdict at current ctx
→ filter: VERIFIED or MEASURED beats ESTIMATED; success_rate > threshold; VRAM ≤ hw profile
→ rank: caller chooses weighting (quality-first vs speed-first vs VRAM-efficiency slider)
→ explain: "Model X is best coding model (93% task success, 142 tok/s, 15.2 GB VRAM, VERIFIED on your 5070 Ti)"
```

Ranking incorporates `releases` relevance (hardware compat + family overlap + quant novelty + `bench.best(model,engine)` delta + recency) — see Signals→Intelligence pipeline below.

---

## Database/Migration Plan

**Today:** `Connection::open` + `CREATE TABLE IF NOT EXISTS` per helper; no `schema_version` table; older DBs pick up new tables lazily on first `ensure_*`.

**Plan (incremental, reversible, no dump-and-reload):**

1. Add `meta` or dedicated `schema_version` guard for cross-cutting migrations.
2. Each new table is additive + `ensure_*` keeps old DBs compatible.
3. Add columns via `ALTER TABLE ... ADD COLUMN` checks (`PRAGMA table_info`) so existing `matrix_runs`/`bench`/`releases` rows remain readable (history survives). `workload_id`, `hardware_profile_id`, `engine_version`, `ttft_ms`, `prompt_tps`, etc. start NULL-able.
4. No destructive normalizations in MVP horizon; avoid moving `matrix_runs` to a new schema in one leap — extend in place, then optionally view/materialize.

---

## API Plan

Existing CLI door stays; Tauri mirrors each via `blocking(spawn_blocking)`:

*Exists:* `scan`, `list`, `fit`, `profile {new,import,list}`, `use [--resident|--managed]`, `engines {list,status,stop,bin}`, `bench {record,list,best,matrix,compare}`, `bringup`, `download`, `feeds {poll,list}` (NEW).

*Near-term:*

* `deck hardware profile` — capture/print `HardwareProfile`.
* `deck workloads {list, add, run}` — manage workloads; `run --workload coding --model X` is sugar over `matrix/compare` with evaluator binding.
* `deck settings {get,set,log,undo} --reason` + `deck feeds rank --workload coding --limit 20` (relevance preview).
* Tauri twins: `hardware_profile()`, `workloads_list()`, `workload_add()`, `settings_get/set/log/undo()`, `feeds_rank()`, `evaluation_list()`.

Contract: business logic in cores; Tauri is serialization only.

---

## Frontend Plan

**Today:** `api.ts` (accepted) sole invoke door; `App.tsx` routes HUD / VAULT / MARKET / LOADOUTS / CONSOLE / BENCH etc.; `dl.ts`/`br.ts` stores; `Vault.tsx` lists models; `Market.tsx` search+org chips+DISK+remote FIT; `Signals.tsx` watchlist; `Hud.tsx`+`PortMap.tsx` residency; `Bench.tsx` grouped scoreboard.

*Next without a megacomponent:*

* Feeds/Intelligence view (from Market table) — LANDED (2026-08-30) as a dedicated FEEDS view: live `feeds_rank` lane with score / `✓ fits` / `why`, workload-hint reweight, and a `feeds_poll` POLL button with busy spinner. Self-fetching, no global state.
* Workload picker: dropdown in Market header + Matrix/Compare form; filters tasks.
* Hardware badge in HUD: `gpu · vram · driver · cyberdeck_ver` + tier badge per resident (ESTIMATED/VERIFIED/MEASURED).
* Settings/audit drawer: `settings get/set` table + undo.
* Keep per-view isolation — no giant `store.ts`; each view fetches via `api.ts`.

---

## Automated Experiment Pipeline

```
Resolve model (local path or HF repo@rev/file) → select best quant(s)
→ download (.part resume, MAX_ACTIVE=2 queue, N-quants via Downloads drawer)
→ inspect metadata (gguf header or safetensors)
→ estimate fit (hardware profile + fit.rs, tier=ESTIMATED)
→ derive config (profile::derive_loadout → ctx ladder, kv, offload, FA)
→ BringUp verify (test port, OOM scan, ctx ladder walk, tier=VERIFIED)
→ bench headless matrix per Workload (isolated cells, keep failures)
→ evaluate (deterministic first; model-judge later)
→ measure (wall_ms, tok_s native/wall, ttft_ms, prompt_tps if available)
→ compare/aggregate → store with hardware/workload provenance + raw output
→ recommend (explainable deterministic query)
→ (opt-in) user clicks TEST THIS MODEL in Market/Feeds — single path above runs end-to-end
```

Which pieces already exist: download `.part`+resume, metadata inspect, fit, derive_loadout, BringUp verify, matrix isolated cells + raw keep, BenchRecord. Gaps: workload binding, TTFT/prompt-TPS/VRAM telemetry, deterministic evaluators, personalized TEST button wiring, recommendation query.

---

## Roadmap Phases

### Phase 0 — Stabilize Existing System (P0, 1–2 days)

*Objective:* Pay down drift before new dimensions; keep the critical download→fit→bringup→bench path green and reversible.

*Why:* Roadmap items are not authorization; correctness over surface (per `AGENTS.md`). Existing 54 Rust tests + vitest must stay green.

*Features:*

* Move `watchlist`/`seen` table creation into `store::open` (or at least `ensure_watchlist_schema` called from `open`) so any door opening the DB gets them, not only `watchlist::open()`. Low-risk consistency.
* Add `schema_version` in `meta` (`key=schema_version, value=int`) + write a helper `migrate_if_needed()` for future alters (no-op today).
* Fix clippy baseline: already 65 accepted warnings — keep `cargo clippy --workspace --all-targets` non-increasing; the 2 `feeds.rs` `non_snake_case` warns now `#[allow]`-ed, rest is baseline drift to not grow.
* Build-smoke: `cargo build` already green; `npm run integrity` 0 violations.

*Code:* `crates/deck-core/src/store.rs`, `crates/deck-feeds/src/watchlist.rs`, `scripts/integrity-rules.json` (accepted line-limit rows already reflect cohesion > count).

*Data model:* `meta(schema_version)` if missing.

*Tests:* Existing 54; add `store::tests::watchlist_schema_survives_open`.

*DoD:* `cargo test --workspace`, `cargo clippy`, `npm run integrity` 0; `deck feeds poll` re-polled twice inserts 0 second run (dedup proven).

---

### Phase 1 — Benchmark Foundation (P0, 3–5 days)

*Objective:* Separate MEASUREMENT from EVALUATION and preserve raw data, so future evaluation layers never need to recompute.

*Why:* Current `MatrixRow` keeps `output` but `bench`/`compare` intermix lexical quality with throughput; you cannot ship deterministic or judge evaluation later without raw provenance.

*Features:*

* Extend `matrix_runs` additively (`ALTER TABLE` if not exists): `workload_id TEXT`, `hardware_profile_id INTEGER`, `engine_version TEXT`, `prompt_tps REAL`, `ttft_ms INTEGER`, `peak_vram_mb INTEGER`, `model_rev TEXT`, `sampling_json TEXT` (temperature/top_p/reasoning). Extend `bench` with `hardware_profile_id`, `engine_version`.
* Extend `MatrixRow`/`BenchRow` structs (nullable new fields); `insert_matrix_run` writes them; older rows read as `None`.
* Enhance `inference::run_prompt` / `health::measure_generation_tps` to capture `ttft_ms` (time from request to first token) and `prompt_tps` when the engine reports it (llama.cpp `prompt_eval_duration`, Ollama `prompt_eval_count/duration`). Fall back graceful. **Reuse existing tooling where it exists:** `llama-bench` / `llama.cpp --bench` and `nvidia-smi dmon` for `peak_vram` are adapters behind the same `GenSample` struct — do not reimplement what the runtime already measures; keep `tok_s_kind` honesty (`native` vs `wall`).
* No evaluation yet — just measurement enrichment.

*Code:* `crates/deck-core/src/store.rs`, `crates/deck-engines/src/inference.rs`+`health.rs`+`matrix.rs`+`compare.rs`, `crates/deck-cli/src/cmd/bench.rs`, `crates/deck-tauri/src/bench.rs`.

*Tests:* `matrix` still records rows even when a cell fails; `ttft_ms` round-trips; `--out matrix.json` includes new fields.

*Dep:* Phase 0.

*DoD:* `deck bench matrix --model ~/models/foo.gguf --tasks "t=hello" --runs 1 --out /tmp/m.json` JSON contains `prompt_tps|ttft_ms|null` + `engine_version` + raw `output`; `deck bench list --json` shows them.

---

### Phase 2 — Workloads & Evaluation (P0, 1–2 weeks)

*Objective:* Make the system answer "what should I use **for this workload**" with credible, pluggable evaluation.

*Why:* Without workloads, benchmarking is workload-blind; lexical quality lets verbose nonsense beat concise correctness. This is the user-visible unlock for the principle.

*Features:*

* `workloads` table + `tasks` table (or `workloads.json` tasks inline for MVP). Seed workloads: `coding`, `reasoning`, `instruction`, `assistant`, `agent` + `custom`. Each workload bundles tasks: e.g. `coding = {gen-test, debug, patch-apply, codegen-unittest}`.
* `Workload` CRUD: `deck workloads list|add|remove`, `deck bench matrix --workload coding` expands tasks from the workload definition (backward-compat with `--task label=prompt`). When `workload` has an `lmeval` mapping, the pipeline transparently delegates to the harness.
* `Evaluator` trait in `deck-core` (or `deck-engines::evaluation`). MVP deterministic evaluators: `exact`, `regex`, `json_schema`, `compile` (run a shell check), `unit_test`/`pytest` + `patch_apply_verify`. Stored as `Evaluation {run_id, method, passed, score, details_json}` linked to `matrix_runs.id`.
* **Leverage existing OSS for the heavy lifting — do not reimplement harnesses:** add `Evaluator::LmEval { tasks: Vec<String>, harness_version }` that shells out to **`EleutherAI/lm-evaluation-harness`** (`lm-eval --model hf --tasks <workload tasks> --output_path /tmp/eval`) via `spawn_blocking` + temp dir + timeout (same isolation as `curl` in `deck-feeds`). Seed mappings: `coding → {humaneval, mbpp}`, `reasoning → {gsm8k, mmlu}`, `instruction → {ifeval}`. Native `exact/regex/json/compile` evaluators remain for repo-local tasks (`pytest`, `cargo test`, `patch_apply`). HELM/OpenCompass are the same pattern later. Store `evaluator_version` in the evaluation row so harness bumps are attributable. Gate on `which lm-eval` — fallback to native evaluators when absent (no hard dep).
* Keep lexical scorer as `evaluator=lexical-placeholder` for non-structured `assistant` tasks where no harness exists — flagged in UI as placeholder, never presented as benchmark truth.
* Matrix records `workload_id` per row; `compare` reads it. Model-judge (`pairwise`/`rubric`) is a later `Evaluator::Judge { model: "qwen3.8:32k" }` arm reusing the isolated test-port harness (judge runs on `ollama` resident, not candidate VRAM).

*Code:* New `crates/deck-core/src/workload.rs` + `crates/deck-engines/src/evaluation.rs` (or `crates/deck-eval` only if it stays tiny — prefer extending `deck-core`/`deck-engines` first per cohesion rule; do not prematurely create a crate), `store.rs` (`workloads`, `tasks`, `evaluations`), `grid.rs` (workload task expansion), `matrix.rs`/`compare.rs` evaluator hook.

*API:* `deck workloads *`; `bench matrix --workload X` sugar; `bench compare --workload X --evaluator exact|regex|json|compile|...`.

*Frontend:* Market/Bench header Workload picker; Bench compare form shows evaluator badge.

*Tests:* Deterministic evaluators unit-tested; `matrix --workload coding --runs 1` writes `evaluation` rows; a 0% pass workload does not produce a best model; model-judge path not required.

*Dep:* Phase 1.

*DoD:* `deck bench compare --model ~/models/qwen --workload coding --engines llamacpp --runs 2` produces a blind report with `passed/score` per trial and aggregate `ok_rate` per candidate, not just lexical score.

---

### Phase 3 — Hardware & Reproducibility (P1, 3–4 days)

*Objective:* Benchmarks become comparable across time, machines, and engine versions.

*Why:* Without hardware profile, a tok/s number is meaningless; VRAM OOM today vs PASS tomorrow is indistinguishable. Ties directly to VRAM trust tiers.

*Features:*

* `hardware_profiles` table + `store::capture_hardware_profile() -> id` (content-hash dedup; runs at bench startup). Fields: `gpu`, `vram_mb` (nvidia-smi total), `cpu`, `ram_mb`, `os` (+ kernel), `driver`, `cuda`, `engine_bin_versions {llamacpp: ver, freetoken: ver, ollama: ver}`, `cyberdeck_ver`, `captured_at`.
* Link `bench` + `matrix_runs` → `hardware_profile_id`.
* VRAM trust badge: UI shows `ESTIMATED` (fit math) → `VERIFIED` (boot succeeded on test port, kept in `matrix_runs.verdict=RUNNING`) → `MEASURED` (bench collected). Prefer `MEASURED` tok_s/ttft when present.
* Optional empirical calibration log: `predicted_vram_mb vs peak_vram_mb` when `nvidia-smi --query-gpu=memory.used` sampled during bench (best-effort, no hard dep).

*Code:* `deck-core/src/hardware.rs` (or inside `store.rs` + `fit.rs`), `deck-engines::health` (capture engine `--version`), `store.rs`, `bench.rs`/`matrix.rs`.

*API:* `deck hardware profile`, `deck bench list` shows `hw#N · driver V`.

*Frontend:* HUD hardware pill; Bench rows link to `hw#`.

*Dep:* Phase 1 (needs enriched measure); parallel with Phase 2.

*DoD:* Two `deck bench matrix` runs days apart with/without driver upgrade produce distinct `hardware_profile_id`s; `bench best --workload coding --json` groups by hardware too.

---

### Phase 4 — Recommendation Engine (P1, 3–5 days)

*Objective:* Deterministic, explainable "what to use" answers per workload on this hardware.

*Why:* Measurement + evaluation without aggregation is just data. Users want a sentence they can act on.

*Features:*

* Pure fn `recommend(workload, hardware_profile, constraints) -> Vec<RankedCandidate>` in `deck-core::recommend`. Aggregates per `(model, quant, engine, profile_name)` over the hardware: success_rate, mean_quality, P50 tok_s/ttft, VERIFIED/MEASURED flag, fit at `ctx` = `derive_loadout` target.
* Weighting presets (not ML): `quality-first` (success_rate → mean_quality), `speed-first` (P50 tok_s), `efficient` (quality per GiB VRAM). CLI flag `--objective quality|speed|efficient` surfaces as a slider in UI.
* Explain line: `"Model X (Q4_K_M) via llama.cpp is your best coding model: 93% task success, 142 tok/s (P50), 15.2 GB VRAM, VERIFIED at 32k ctx on your 5070 Ti (hw#4, driver 555) — 2.1× faster than your current fallback."`
* Powers Market personalization.

*Code:* New `crates/deck-core/src/recommend.rs`, `store` aggregates query, `deck-cli/src/cmd/recommend.rs` (or `bench best --workload`), `deck-tauri::recommend`.

*API:* `deck recommend --workload coding [--objective quality] [--json]`, `deck bench best` extended with `workload` filter.

*Frontend:* Bench scoreboard groups by `(model×engine×workload)` + explanation banner; Hud header "best per workload" chips.

*Tests:* Recommend over an empty DB returns "insufficient data, run `matrix --workload`"; over seeded `matrix_runs` picks the deterministic winner.

*Dep:* Phases 2–3.

*DoD:* User can run `deck recommend --workload reasoning` and get an explainable ranked list that matches `deck bench compare` aggregates.

---

### Phase 5 — Online Intelligence (P0 track, parallel with Phases 1–3, split Horizons)

*Objective:* Turn online polling from a firehose into ranked, relevant discoveries.

*Horizons already in `feature-parity.md`:*

**O1 DONE (this session):** `Source` trait + `hf`/`github` adapters, `releases` dedup store, `deck feeds poll/list`, Tauri `feeds_{poll,list}`.

**O2 — Relevance scoring (S, next):**

* Pure `score(Release, hw_profile, installed_models, bench.best) -> f32` in `deck-core::relevance` with `w1·fits_hardware (estimate() PASS/WARN/OOM) + w2·family_overlap (installed arch/quant) + w3·quant_novelty + w4·bench_delta (does a better quant of same family exist?) + w5·recency`. Sort Market/Feeds by score per workload hint.
* `deck feeds rank --workload coding --limit 20` + Tauri `feeds_rank`.

**O3 — Settings + Audit (S):**

* `settings` (`key TEXT PK, value_json TEXT, updated_at`) + `audit_log(ts, actor, key, old_json, new_json, reason)` in `store.rs`. `deck settings get/set --reason msg` (+ `log`, `undo <ts>`). Typed validation (intervals, enabled_sources, thresholds). Agent writes go through this API — never raw file edits. Covers `AGENTS.md:18–19` requirement.

**O4 — What changed / What to test (S):**
* HUD/SIGNALS "New→Relevant→Worth testing" lane: releases since `fetched_at > last_seen`, filtered to `score > threshold`, enriched with `FIT at ctx`, `DISK`, `tok/s of current best equivalent`. Answers "what changed / what matters".
  **Partial LANDED (2026-08-30):** a dedicated FEEDS view (`frontend/src/views/Feeds.tsx` + route) surfaces the live `feeds_rank` pipeline — hardware-grounded score, `✓/✗ fits`, and the `why` reasons behind each candidate, with a workload-hint selector (coding/reasoning/… reweights family overlap) and a POLL button. **Recency gate also LANDED (2026-08-30):** a `feeds.last_seen` epoch setting (written through the audit-tracked O3 settings store, actor `ui`) drives a persistent NEW marker computed as `fetched_at > last_seen`; "MARK SEEN" is the only thing that advances it, so "what changed since I last looked" survives routine polls. Still open from O4: automatic DISK/fit-at-ctx enrichment in the lane.

**O5+ (Horizon 2)** deferred: background polling timers, notifications (HUD badge + tray), caching/ETag/429 per source — not in this MVP block.

*Code:* `deck-feeds/*`, `deck-core::relevance`+`store(releases,settings,audit)`, `deck-cli::feeds`, `deck-tauri::feeds`, frontend Feeds/Market rank column.

*DoD O2–O4:* `deck feeds poll && deck feeds rank --workload coding` ranks HF GGUFs by whether they PASS at 32k on this 5070 Ti, not by global downloads/popularity.

---

### Phase 6 — Automated Experimentation (P1, 1–2 weeks, after Phases 2–5)

*Objective:* Single-click `TEST THIS MODEL` closes the loop Download→Fit→BringUp→Workload→Evaluate→Recommend without manual flag surfing.

*Features:*

* One typed pipeline `experiment::run(repo, workload, engine)` that reuses: `market::model_files` (resolve quant/file) → `download` resumable queue → `gguf` header inspect → `fit+derive_loadout` → test-port verify → `matrix` (via bench harness) with workload tasks + evaluator (`lm-eval` when `workload` maps to harness tasks, otherwise native) → persistence with hardware profile → relevance update.
* Market/Feeds row action: `TEST` button + quant picker → invokes `experiment_start` Tauri command with progress stream (reuse `bringup` event bus shape, `experiment-*` events) → on completion writes `matrix_runs` + `evaluations` + shows recommendation delta ("WORSE/BETTER/UNKNOWN vs your current best").
* Failure is data: boot failures → `matrix_runs.verdict=CRASH/OOM`, evaluators store `passed=false`, candidates surface `⚠ CRASH` not silent zero.

*Code:* `deck-engines::experiment` (new) or extend `matrix`/`bringup`/`download` orchestration, `deck-cli::experiment`, `deck-tauri::experiment` with single-flight guard (`EXPERIMENT_RUNNING`).

*API:* `deck experiment run --repo unsloth/Qwen-GGUF --workload coding --engine llamacpp` (long-running).

*Dep:* Phases 1–5 (needs workloads+hardware+scoring+feeds).

*DoD:* From Market, user clicks TEST on a WORTH_TESTING release → without editing flags, obtains a new `matrix compare`-grade report that updates `deck recommend --workload coding` if it actually beats the incumbent.

---

### Phase 7 — Agent Operator (P1, staged, parallelizable)

*Staged exactly as the permission ladder demands:*

* **7a READ/ANALYZE (S, after O2–O4):** Typed tools the agent can call: `hw_info`, `list_models`, `feeds_list/rank`, `bench_history`, `workloads`, `recommendations`. No shell. Always allowed. Ship as MCP/tools surface via Tauri/CLI JSON. Unlocks "Relevance Analysis" without risk.

* **7b MODIFY CYBERDECK (S, after O3):** Agent `settings set/get`, `profile` mutations, workload config via typed APIs. Each write goes through `settings` validation + `audit_log` (who=agent, reason required), undoable. This is `AGENTS:18` contract.

* **7c EXECUTE CONTROLLED (M, after Phase 6):** Agent can `feeds poll`, `download`, `bench matrix/compare`, `bringup`, `experiment run` via typed tools — each requiring explicit user consent flag (`--allow-execute`) and rate-limited/disk-guarded. High-risk (delete, rewrite units outside deck, autonomous loops) stays blocked.

* **7d Agent UX (S):** Console surfaces `agent tool call: recommend(coding) → result` telemetry so the user sees what the agent did while away.

*Dep:* O3 for audit; Phase 6 for safe execution.

*DoD 7a–7b:* An agent can explain "why TEST X" without calling `bash`; `settings log` shows agent edits revertible. 7c gated by consent.

---

### Phase 8 — Canvas & Workflow Orchestration (P1→P2, after Phase 7, after HUD multi-agent)

*Objective:* HUD becomes an **infinite canvas of moveable TUIs** (like `opencode`’s own TUI, but per-agent, per-model) and then a **node-based workflow canvas** where a `reviewer` agent spins up on condition with a different model than the `coding` agent — the ambitious idea from 2026-08-30.

*Why now vs later:* Fleet (`PORT MAP` residency, `fit`, `bench` + `hardware`) is now stable (99 tests green) — the next hard part is **workload-aware routing** (`which model for which node`), which is exactly `Phase 2 workloads + Phase 4 recommend` we just landed. A canvas without that routing is just a window manager. This phase reuses the same `console.rs` multiplexer (`opencode run` sessions by `id`) but surfaces it as spatially free TUIs.

*Features (incremental, reordered per `CANVAS.md` — headless foundation before canvas UI):*

* **8a — Draggable TUIs in HUD (DONE earlier):** Each `HUD` session becomes a draggable card with per-card model picker, plus embedded `opencode attach` PTY panes (`tui.rs`) — the spatially-free primitive the canvas builds on. Persists `canvas_layout`.

* **8b — ~~reactflow CANVAS~~ → superseded:** The original 8b ("CANVAS view with `reactflow` + `workflows` table") is folded into the cleaner split below (8c foundation → 8d UI → 8e matrix → 8f branch/loop). `reactflow` is deferred to 8d as an optional renderer swap, not a dependency.

* **8c — Workflow Foundation — DONE (headless):** `Role` ⟵ `ModelBinding` (a node is a role bound to a model), `Node`/`Edge`/`Workflow`/`Run`/`NodeRun`/`Message` data model, **pure DAG scheduler** (`deck_core::workflow::plan`: wavefronts, fan-in, `has_cycle`, unreachable), persistence tables (`roles`, `workflows`, `workflow_runs`, `node_runs`, + `role_id` on `matrix_runs`), and a runner-agnostic **headless executor** in `deck-engines::workflow` (`execute` walks waves; `StatelessRunner` via `run_prompt`; `AgenticRunner` via headless `opencode run`). CLI door `deck workflow {save,list,run,history}` landed + smoke-tested (DAG drives both nodes, errors → `Partial`, history persists). Schema version gate landed (Phase 0).

* **8d — Canvas UI shell — LANDED (2026-08-30):** minimal DOM canvas rendering saved workflows; per-node positioned cards (roles bound to models) + SVG edges for the graph, and a RUN/STOP door that fans out to the Tauri background executor behind `wf-*` events. `reactflow` stays an optional renderer; xterm.js panes come with the fuller 8d (agentic node sessions). Tauri twin landed: `deck-tauri/src/workflow.rs` (background run registry + `wf-*` events + `workflow_{seed,save,list,get,run,stop,history}`), registered in `src-tauri` + `api.workflow*`, with a headless persistence test. `Run workflow` now drives the DAG end-to-end from the CANVAS view.

* **8e — model matrix / per-role bench — LANDED (2026-08-30):** a workflow the user has run against several models accumulates per-role benchmark rows (`role_id` on `matrix_runs` feeds Phase 4 recommend) so the canvas can show "which model best at which node". `NodeRunner::run` now returns a `NodeOutcome` (text + tps/ttft/gen_tokens) so the executor threads generation metrics instead of discarding them; both doors record a `matrix_runs` row per engine-backed node on every run (`node_to_matrix_row`), and `store::per_role_bench` aggregates best/avg/last tok/s per role+model (NULL-tok_s agentic rows excluded). Surfaced as `deck workflow bench <id>` + Tauri `workflow_per_role_bench` + a PER-ROLE BENCH table in the CANVAS view (BEST badge per role).

* **8f — branch / loop — LANDED (2026-08-30, branch + loop; supervisor deferred):** conditional routing and a bounded loop construct, both on serialized (never-executed) predicates.
  - **Branch** (`WorkflowEdge.condition`): a safe, code-free predicate language (`EdgePredicate`: `contains:`, `not_contains:`, `starts_with:`, `is_empty`, `not_empty`, `always`) evaluated against a node's produced text. A downstream node is **skipped** when *every* incoming edge is a conditional that evaluated false — the "reviewer on condition" pattern. Unconditional fan-in is byte-for-byte backward compatible.
  - **Loop**: an explicit `loop_edge` back-edge (never a raw cycle — the scheduler refuses those) whose (continue) predicate closes the body; re-execution is bounded by `ExecSettings.max_iterations` (0 = disabled, validated) and the token budget. The loop source's output is carried back into the loop target as an input, enabling "Dev ⟲ Reviewer until DONE". `ExecReport` reports `iterations`; skipped nodes carry `NodeResult.skipped`.
  - **Supervisor** (spawn/retry sub-workflows) is deliberately deferred — out of this session's scope.**

*Code:* `crates/deck-core/src/workflow.rs` (domain + DAG scheduler), `crates/deck-core/src/wfstore.rs` (persistence), `crates/deck-engines/src/workflow.rs` (executor + runners), `crates/deck-cli/src/cmd/workflow.rs` (CLI door); LANDED: `crates/deck-tauri/src/workflow.rs` + `src-tauri` `workflow_*` commands (background runs + `wf-*` events) — the Tauri twin of the CLI door, reachable from the 8d CANVAS view. `deck workflow save` seeds the `residents`-agnostic Coding Review template this UI renders.

*API:* `deck workflow {save --seed|--file, list, run <id> --runner stateless|agentic [--dir] [--model], history [<id>]}` (landed); LANDED `deck-tauri` `workflow_{save,list,get,run,stop,history}` + `wf-start/wf-node/wf-done/wf-error` events, frontend `api.workflow*`.

*Frontend:* LANDED (8d shell): `frontend/src/views/Canvas.tsx` + route in `App.tsx`.

*Tests:* (landed) DAG scheduler linear/fan-in-fan-out/cycle; wfstore role+workflow+run round-trips; executor fan-in payload, stop, runner errors; CLI smoke (seed→list→run→history); Tauri `execute_and_persist` headless two-node chain. 8f adds: EdgePredicate parse/eval; scheduler accepts loop back-edges + rejects raw cycles; executor branch-skip + budget; bounded loop terminate-by-predicate / cap-by-max_iterations / stop-by-token-budget; backward-compat unconditional-never-skips. Pending: 8d canvas drag persists; supervisor (deferred).

*Dep:* Phase 2 workloads + Phase 4 recommend (to pick per-node model) + Phase 7a `agent_tools`; HUD concurrent sessions are green.

*DoD (8d target):* User runs `deck workflow save --seed`, opens `CANVAS`, drags the `coding-review` DAG nodes, `Run workflow` → `coder` emits → `reviewer` auto-spins on `qwen3.6 :1919` with its own `xterm` log, both visible and movable.

*DoD (8f landed):* a workflow with `Edge.condition` + a `loop_edge` back-edge (e.g. `dev → rev ⟲ rev` with `not_contains:DONE`, capped by `max_iterations`) imports via `deck workflow save --file`, renders loop/condition markers in CANVAS, and runs end-to-end: the body re-executes while the predicate holds and stops on the terminate predicate, the token budget, or the iteration cap — with skipped downstream nodes reported but not benchmarked.

---

### Phase 9 — Autonomous Daily Driver (P2→P3, long-term, explicitly not MVP)

*Objective:* `Cyberdeck left running → morning report` (What changed? What matters? What should I test? What should I use? Is anything broken? What did you do while I was away?)

*Deferred by design:*

* Persistent background service / systemd user timer polling feeds + ranking.
* Opt-in `IF new release score > threshold AND predicted delta > X THEN experiment run automatically` (Horizon 3 O9 in `feature-parity.md`).
* Self-optimization regression investigation (Phase 10 in O10): detect `perfDelta -18% → inspect engine/driver/model/config/VRAM pressure → propose reversible change → re-bench → keep/revert`.
* Empirical hardware profiling calibration, power efficiency, multi-GPU, remote-machine benchmarking, community benchmark datasets, automatic workload generation.

*Guardrail:* No daemon, no autonomous spend of disk/VRAM, no self-modification of source code without heavily permissioned explicit consent — ever (`AGENTS:19`).

---

## Priority Matrix

| Feature | Impact | Effort | Risk | Priority | Notes |
|--------|:------:|:------:|:----:|:--------:|-------|
| Phase 0 stabilize (schema_version, watchlist fix, integrity) | H | XS | Low | **P0 DONE** | `schema_version` guard + `meta` table landed; watchlist/integrity remain |
| Phase 1 measure enrich (ttft/prompt_tps/hw link) | H | S | Med | **P0** | Unlocks workloads+hardware; additive migrations |
| Phase 2 workloads + deterministic evaluators | H | M | Med | **P0** | The workload unlock; keep lexical flagged |
| O1 feeds foundation (Source+releases catalog) — **DONE** | H | S | Low | P0 DONE | Landed this session |
| O2 relevance rank | H | S | Low | P0 | Makes Market personal |
| O3 settings+audit typed API | H | S | Low | P0 | Agent safety prerequisite |
| Phase 3 hardware profile + VRAM trust tiers | H | S | Med | P1 | Gives bench provenance; defer if tight |
| Phase 4 deterministic recommend | H | S | Low | P1 | Explainable sentences; no ML |
| Phase 6 one-click experiment pipeline | H | M | Med | P1 | Closes the loop; depends on 1–2 |
| Phase 7a–b agent READ/ANALYZE/MODIFY | M | S | Low | P1 | Safe agent value; shell-free |
| O4 HUD what changed lane | M | S | Low | P1 | FEEDS view + recency gate landed; DISK/fit-at-ctx enrichment open |
| Phase 8 Canvas — draggable TUIs (8a) | M | S | Low | P1 DONE | HUD multi-agent, zen+local side-by-side, embedded `opencode attach` TUIs |
| Phase 8 Canvas — workflow foundation (8c) | M | M | Med | P1 DONE | Role/Model/DAG scheduler + headless executor + `deck workflow` CLI; Phase 0 schema gate |
| Phase 8 Canvas — UI shell (8d), matrix (8e), branch/loop (8f) | M | M–L | Med | P2 | 8d+8e DONE (canvas + per-role bench); 8f branch+loop DONE, supervisor remains |
| Phase 7c agent EXECUTE (consented) | M | M | High | P2 | Needs experiment + audit solid |
| Phase 9 daemon/autonomous/self-heal | M | M–L | High | P3 | Long-term; do not start early |

*Ruthless cuts:* Cross-engine N-GPU VRAM tricks, cloud benchmarking, distributed inference, social feeds, extra dashboards, ML recommender — all P3 *never before P0–P1 green*.

---

## Refactoring Plan

| Module | Problem | Why | Change | Risk | Migration |
|--------|---------|-----|--------|------|-----------|
| `store.rs` (770→ growth) | Megafile near 600 hard limit (accepted debt: 5 STORE/PROFILE/BRINGUP/TEST rows already accepted); now adding releases→settings→hardware→workloads | Gate soft-limit churn | Extract `store::{releases,hw,workload,settings}` submodules under `store/` but keep one `Connection::open` + single `ensure_*` aggregator; file move only, no logic change | Low | Move `ensure_releases_schema+Release` etc. to `store/releases.rs`, re-export; `integrity-rules.json` line-limit adjusted incrementally |
| `gguf.rs` 699 parked | Known debt, TRUNC-truncated header heuristic; GQA fix landed last session but header fetch still 2 MiB cap without telemetry | Fit correctness | On next touching change, split parser-vs-Range plumbing only; keep parser pure. No rewrite without measured bug | Low | Parked tripwire already |
| `matrix_runs`/`bench` schema | History survival not guaranteed without `PRAGMA` + ALTER guard | Lost history | Add `schema_version` + `ensure_column` helper before Phase 1 extends | Low | **RESOLVED** — `schema_version` gate + `ensure_column` landed; `role_id` added additively (Phase 8c) |
| `inference.rs` 253 lines | OpenAI probe conflates sampling/ttft/reporting; engine protocol is registry but sampling params scattered | Measure enrich | Introduce `Sampling { temp, top_p, ... }` + `GenSample { prompt_tps, ttft_ms, ... }` returned from harness; pure struct | Med | Extend `GenSample` backward-compat (Option new fields) |
| `Console` opencode passthrough | Shell-lean agent with ambient perms | Block Phase 7 safety | Introduce `agent_tools` Tauri registry (READ/ANALYZE typed verbs) before granting EXECUTE | Low→High if skipped | Gate EXECUTE behind `allow_execute` flag + audit |

No rewrite of `fit` estimation, `scanner`, or `systemd` generation — they are the moat.

---

## Things Not To Build Yet (Explicit)

* **Engine proliferation** — `Engine::all()` is the gate; adding `vLLM`/`TensorRT-LLM`/etc. before workloads+hardware+recommend triad is vanity (one `EngineDescriptor` + `inference` arm per real need only).
* **Cloud/remote benchmarking** — local VRAM+VRAM trust is the identity; remote fleet breaks the premise.
* **Distributed / multi-GPU inference** — irrelevant to the 5070 Ti ground truth (16 GB VRAM, 30 GB RAM). Track but do not optimize for.
* **ML recommender** — deterministic `quality+s tok_s+VRAM` rank explains itself; ML adds opacity before evidence exists.
* **Social / community benchmark datasets** — requires stable hardware profiles and schema first (Phase 8 horizon).
* **Excessive dashboards / Documents/Email from Odysseus parity SKIPs** — `feature-parity.md` correctly SKIPs document editor, email, notes/calendar/gallery/auth; re-introducing them crowds the driver.
* **Custom mega-store / orchestration framework** — extend `dl.ts`/`br.ts` stores incrementally; do not introduce a giant frontend state object.

---

## MVP Definition

*Smallest genuinely useful loop — not "every feature shipped":*

> **User can, for a named workload on their actual hardware, answer "what should I use and why" from measured evidence.**

* User picks **Workload = coding** (Phase 2 seed) and runs `deck bench matrix --model ~/models/qwen --workload coding` (or Tauri Compare with workload picker).
* Benchmarks record raw `output` + deterministic `passed/score` + `ttft_ms/prompt_tps` + `hardware_profile_id` + `engine_version` (Phases 1–3).
* `deck recommend --workload coding` (Phase 4, deterministic) returns a ranked list with one explainable winner line citing `success_rate + tok_s + VRAM + VERIFIED/MEASURED`.
* Feeds already polling (O1 DONE); O2 `feeds rank` surfaces personal `WORTH_TESTING` candidates for that hardware; user sees *why* TEST would matter.

**MVP input:** one workload + two quants of the same family on one engine (llamacpp) on one hardware profile. **MVP output:** one sentence recommendation backed by `matrix_runs` evidence. Everything else (multi-engine, model-judge, autonomous) is post-MVP.

---

## Long-Term Vision (Post-MVP, keep separate)

* Continuous model discovery + automatic overnight candidate bench (opt-in) → regression detection (18% drop → re-test → keep/revert) → empirical VRAM calibration (`predicted vs measured` per model family).
* Power-efficiency (`tok/s per watt`), multi-GPU sharding, remote-machine delegation (Cyberdeck as control plane for a second box).
* Community anonymized bench datasets + model retirement advice ("your coding model is 3 releases behind the family's best — X now beats it by 22% success rate").
* Automatic workload synthesis (repo-mined coding tasks) + agent-driven maintenance (agent periodically `feeds poll` → `rank` → `experiment` when consented).

None of this is P0. Each depends on stable Workload+Evaluation+Hardware+Recommend layers.

---

## If This Were My Project (Next 8, in order)

**1. O2 — Relevance scoring (deck-core::relevance, deck-cli `feeds rank`)**
*Where:* `crates/deck-core/src/relevance.rs` + `crates/deck-core/src/store.rs` (extend `releases` query), `crates/deck-cli/src/cmd/feeds.rs`, `crates/deck-tauri/src/feeds.rs`, `frontend/src/views/Market.tsx` rank column.
*Why now:* O1 already lands the catalog; without ranking, Market is still `"what's popular?"` not `"what's worth testing on your 5070 Ti?"` The scoring fn is pure (hardware+fit+bench deltas), ships without migrations, and immediately makes the feeds pipeline demoable.
*Unlocks:* Personalized Market (Phase 5 O4) and Recommend (Phase 4) have signal to query.
  **Hardening (2026-08-30, hardware-is-truth):** the offline `hw_term` size guess was a blind name-substring heuristic with an `else 8.0` default — it declared the 125B-total MoE `Qwen3.8-Flash-Next` "~8GB ✓ fits" (it does NOT fit this 16 GB box). Now `params_total_b` parses the repo name: integer totals (`28b`, `70b`, `35b`, …) size it; decimal/composite MoE names (`3.8`, `1.5`) and names with no `NNb` marker are *uncertain* → `fits:false` with "probe GGUF in MARKET before testing", so the rank never overclaims a fit for an un-sizable flagship. Reminder: real fit still comes from `fit::estimate` on the actual GGUF header via MARKET's `browse_fit_remote`.

**2. Phase 1 — Enrich measurements (ttft_ms, prompt_tps, engine_version)**
*Where:* `crates/deck-core/src/store.rs` (`ALTER ADD COLUMN` + `MatrixRow`/`BenchRow` options), `crates/deck-engines/src/inference.rs`+`health.rs` (`GenSample` extension), `crates/deck-engines/src/matrix.rs`, `crates/deck-cli/src/cmd/bench.rs` `--out` JSON, `crates/deck-tauri/src/bench.rs`.
*Why now:* Workloads+Recommend cannot be retrofitted credibly without provenance; adding columns later breaks history.
*Unlocks:* Phases 2–4 have raw data to aggregate.

**3. Phase 2 — Workloads + deterministic evaluators (coding workload first)**
*Where:* `crates/deck-core/src/workload.rs` + `crates/deck-engines/src/evaluation.rs`, `store.rs` (`workloads/tasks/evaluations`), `grid.rs` (expand `--workload coding`), `matrix.rs`/`compare.rs` evaluator hook, CLI `workloads` + `bench --workload`, frontend Workload picker.
*Why now:* This is the vision's central new domain concept; delay inflates placeholder-score drift.
*Unlocks:* MVP's "what for this workload" sentence becomes testable.

**4. O3 — Typed Settings + Audit Log (blocking Agent safety)**
*Where:* `crates/deck-core/src/settings.rs` (or `store.rs` extension), `store.rs` (`settings` + `audit_log` tables), `crates/deck-cli/src/cmd/settings.rs` (`get/set/log/undo --reason`), `crates/deck-tauri/src/settings.rs`, frontend Settings drawer.
*Why now:* Agent MODIFY/EXECUTE cannot ship safely without audited, revertible settings; Phases 7b/7c gate on this.
*Unlocks:* Agent ladder 7a→7b transition with undo.

**5. Phase 3 — Hardware profiles + VRAM trust tiers**
*Where:* `crates/deck-core/src/hardware.rs`, `store.rs` (`hardware_profiles`), `deck-engines::health` (capture `engine --version`), `store::capture_hardware_profile()` at bench startup, HUD badge `ESTIMATED/VERIFIED/MEASURED`.
*Why now:* Minor deps, high explainability payoff; bench numbers become comparable.
*Unlocks:* Recommend's "on your 5070 Ti hw#4" clause and future regression detection.

**6. Phase 4 — Deterministic Recommend**
*Where:* `crates/deck-core/src/recommend.rs`, `store` aggregate queries, `crates/deck-cli/src/cmd/bench.rs` (`bench best --workload`) or `recommend.rs`, `deck-tauri::recommend`, `frontend/src/views/Bench.tsx` explanation banner.
*Why now:* Converts Phase 2 evaluation into a one-sentence action; no ML needed.
*Unlocks:* MARKET `Recommendation: TEST/BETTER` personalization (ties to O2 score).

**7. O4 — Signals→Intelligence "what changed" lane**
*Where:* `frontend/src/views/Signals.tsx` (rename or split to Intelligence) + `Hud.tsx`, consuming `feeds_rank` + `FIT at ctx` + `DISK` + current-best `tok/s`.
*Why now:* The daily-driver promise needs a surface; reuses O2+Phase 4 data, no new backend depth.
*Unlocks:* The "open Cyberdeck and know what matters" moment for manual daily drivers before automation.

**8. Phase 6 (first slice) — One-click experiment wiring for one quant→one workload**
*Where:* `crates/deck-engines/src/experiment.rs` (orchestrates `download→inspect→fit→derive→verify→matrix(eval)`) with single-flight guard, `deck-tauri::experiment_start` (`experiment-*` events like `bringup-*`), `frontend/src/views/Market.tsx` TEST button + progress drawer.
*Why now:* Completes O1→O4→Phase 6 loop end-to-end (Resolve→Test→Recommend) for the MVP dataset without yet building ModelJudge or daemon.
*Unlocks:* The flagship `TEST THIS MODEL` demo — the entire vision compressed into one button.

---

*This plan is implementation-ready: every next step names the crate/file, the exact tables/APIs/views it touches, its dependency chain, and its DoD. No re-architecture, no giant service, no premature ML — just the measured evidence loop the product principle demands.*
