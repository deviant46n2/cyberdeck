# Feature Parity + Direction: cyberdeck vs. Odysseus

Tracking doc: make cyberdeck a **chat-for-everything workspace** with
[Odysseus](https://github.com/odysseus-dev/odysseus) (PewDiePie's self-hosted AI
workspace, AGPL) as the feature bar to meet, then beat it on its actual reason
to exist — model-fleet control, VRAM fit, and **benchmarking** with llama.cpp
and FreeToken as first-class residents.

The rule: cyberdeck should do everything Odysseus does **at least as well**, and
beat it outright on the model-fleet / VRAM-fit / throughput-measurement axis.
Where a feature is "same-but-better" we mark status `DONE` (already better) or
`PORT` (reuse Odysseus approach, adapt to our stack). Where it's a whole new
surface we don't care about, we don't build it.


---

## Direction (2026-08-27)

cyberdeck is **not** just a model-fleet manager with an agent bolted on. It is a
**chat-for-everything workspace where the loadout machinery is the runtime
underneath chat — and benchmark data is what tells you which loadout to be in.**

That means three architecture commitments that shift how features land:

1. **Multi-model residency, not single-unit swaps.** Today `deck use` swaps ONE
   active systemd unit at a time, so the alias contract holds on `:18000`/`:1919`.
   "Chat for everything, run multiple at once, flip between loadouts a lot"
   breaks that assumption — you want **several engines resident on their own
   ports** (llamacpp / freetoken / ollama, each a distinct model) and chat
   routing per-message to whichever is up, not swapping one service.

   -> Introduce a **PORT MAP**: each engine has a fixed port slot
   (:18000, :1919, :11434, ...) and an optional profile bound to it. `deck use`
   becomes "bind profile X to slot :PORT and start it" — which may run
   *alongside* other ports. Downstream clients (opencode/dsh) pick a slot, and
   rewire already points them at the right baseURL. The single-unit swap stays
   as the default, but a "resident" flag keeps N engines running concurrently.

2. **Concurrent chat sessions across residents.** HUD/CONSOLE already run N
   concurrent *agent* sessions; generalise so each session pins an
   engine-slot+model, streams tokens (not just opencode ANSI lines), and is
   swappable live. "Flip between loadouts a lot" = retarget the *next* message
   without killing the resident.

3. **Recommend where to be, from data.** The benchmark DB feeds the chat
   header: show tok/s + fit for each resident so you can see before you type
   whether to say it to qwen @ :18000 or freetoken @ :1919.

With that framing, the parity table below is ordered by **what makes the
workspace feel complete as a chat surface first**, then benchmark depth.

---

## Flagship flow: "plug in a model, click the engine, done"

The north-star interaction. Stop thinking in settings panels; think in intent:

> **Pick a model → click FreeToken → cyberdeck figures out the max context
> window, spins up the prefill/offload server, and brings it up.**

"Click FreeToken (or llama.cpp) and it does the rest" decomposes into a
**single `BringUp` command** that derives first, then verifies on a test port,
then installs — so the user never touches oo, n_gpu_layers, ubatch, flash-attn,
KV-qtype, MoE cache, or reasoning flags by hand.

### BringUp(model, engine) → steps

1. **Fit at max context.** From the model's GGUF header (`n_layers`, `n_embd`,
   quant) + detected VRAM (`hw_vram()`), run `estimate()` to find the largest
   `ctx_size` that still `PASS`es. That becomes the derived ctx. (Reuse
   `deck-core::fit` + the BROWSE remote-fit path.)

2. **Derive the full profile** from that fit (this is the new logic, in
   `deck-engines`):
   - `ctx_size` ← the max that PASSes (step 1)
   - `n_gpu_layers` ← 0 when it all fits on GPU; else 64 (all-in) with
     freetoken `offload` spilling to RAM
   - `flash_attn: true`, `kv_cache_type_k/v` ← from the fit's kv_bytes
   - FreeToken: `ft_backend` + `ft_moe_cache_size` prefill/offload server; the
     "pregen server" the user mentions = the offload worker, brought up on its
     own slot
   - llama.cpp: MTP `spec_type`/`draft_model` if a draft model is present;
     `reasoning_budget` high for reasoning models
   - `ctx_ladder` ← steps below the max (so a real-world OOM still degrades
     gracefully instead of failing the bring-up)

3. **Verify headlessly (never touch the live service).** Reuse the test
   harness: stop live, spawn on the test port (`LlamaCpp 18999` /
   `FreeToken 18998`), `health_ok_any` poll, `/metrics` check, OOM-scan the
   log. If it OOMs or doesn't reach health, walk `ctx_ladder` down and retry.
   This is exactly the autotune idea, but **one-shot toward max ctx** by default.

4. **Install + start** on the engine's real port slot, `render_unit` → `.bak`
   → `systemctl` → health-wait (reuse `deck use` machinery).

5. **Bench it and record.** `/metrics` → tok/s into `cyberdeck.db`, so the chat
   header (B3) can show "FreeToken · qwen3 · 178K · 100 tok/s · PASS" after
   bring-up.

Net effect: **model + engine menu → working chat on the best-max-ctx config in
a few seconds, with a benchmark row to prove it.** No flag surfing.

Build as: `deck bringup --model <path> --engine freetoken [--dedicated-port]`
CLI first (headless-tested like everything else), then a HUD/Chat **"LOAD"
button** that calls it.

---

## Scoring legend

| Status | Meaning |
|--------|---------|
| `DONE` | cyberdeck already has this, and does it as well or better |
| `PARTIAL` | core exists, gaps to close (listed) |
| `PORT` | Odysseus approach worth adapting to cyberdeck's Rust/TS/Tauri stack |
| `SKIP` | deliberately not building — see note |
| `EXTEND` | exists but we make it meaningfully stronger/benchmark-aware |

---

## Odysseus feature surface

### 1. Chat + Agents — local/API models, tools, MCP, files, shell, skills, memory

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Multi-provider chat (local + API) | HUD harness + engine status | `PARTIAL` | We serve llamacpp + FreeToken only. Add generic OpenAI-compatible provider pick (Ollama present in `opencode.json`). |
| Autonomous coding agent | CONSOLE / HUD `opencode run` sessions | `DONE` | Streaming multi-session, `--auto`, per-session stop. Already better (bench-aware model pick). |
| Tools / shell inside agent | inherited from opencode | `DONE` | Agent has read/edit/bash/task. |
| MCP servers | none | `PORT` | opencode supports MCP; expose a per-session MCP picker so agents get DB/knowledge tools. |
| Files upload / attachment | none | `PORT` | Add file attach to HUD prompt → passes context into `opencode run`. |
| Skills | none (outside opencode's) | `PORT` | Surface `~/.config/opencode/skills/*` as selectable skills in the agent panel. |
| Memory | none | `PORT` | Wrap `opencode`/deck state so agents can persist recall; also log sessions to `cyberdeck.db`. |

### 2. Cookbook — hardware-aware model recommendations, downloads, serving

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Hardware-aware model recs | fit estimator + `hw_vram()` | `PARTIAL` | We compute fit for a *given* model. Gap: **suggest** models that fit your VRAM. Close via remote GGUF header fetch across MARKET/BROWSE → rank by fit. |
| Downloads | MARKET → `~/models` | `DONE` | GGUF HEAD-resolved sizes, one-click download. |
| Serving | systemd units, `deck use` | `DONE` | Alias+port contract, ctx ladder, `.bak`. Stronger than Odysseus. |
| Quant-aware guidance | GgufMeta parse | `PARTIAL` | We know quant. Add "best quant that still PASSes" inference. |

### 3. Deep Research — multi-step web research + report generation

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Deep research agent | none | `PORT` | Ship a research *skill*: prompt the agent with a recursive "search → read sources → synthesize → report" loop. Benchmark-aware target. |

### 4. Compare — blind side-by-side model testing + synthesis

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Blind A/B model compare | none | `EXTEND` | **This is cyberdeck's home turf.** Build a `deck bench compare` that runs the same prompt(s) across N loadouts, captures tok/s + output, blind-randomizes which is which, then scores. Feed results into the benchmark DB. |
| Synthesis of comparison | none | `EXTEND` | Post-compare synthesis via the agent (same harness). |

### 5. Documents — writing-first editor with AI edits

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Document editor with AI edits | none | `SKIP` | Off-topic for a model fleet manager. Agents already edit files. Not building a full editor. |

### 6. Email — IMAP/SMTP triage

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Email inbox + triage + replies | none | `SKIP` | Not benchmark-centric. Out of scope. |

### 7. Notes, Tasks + Calendar

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Notes / todos / reminders / CalDAV | none | `SKIP` | Out of scope for model management. |
| Scheduled agent tasks | none | `PARTIAL` | **Keep a sliver**: auto-run `deck bench` + SIGNALS check on an interval (cron-style). Watch-only, benchmark-facing. |

### 8. Extras

| Odysseus | cyberdeck | Status | Notes |
|----------|-----------|--------|-------|
| Web search | HF feed retrieval | `PARTIAL` | Agent inherits websearch; add model-release web search to SIGNALS/MARKET. |
| Themes / sessions / presets | synthwave theme, loadouts | `DONE` | Loadouts = presets + sessions. Theme fixed by design. |
| Gallery / image editor / uploads | none | `SKIP` | Not relevant. |
| 2FA / auth | none | `SKIP` | Local desktop app, single user. Not needed. |

---

## cyberdeck advantage layer (what Odysseus can't do)

These are the benchmark-centric differentiators that make cyberdeck worth
having at all. They are the reason this project exists, not Odysseus parity.

### B1. VRAM fit engine
- Exact GGUF header parse (EOF-tolerant Range fetch) → `n_layers`/`n_embd`/quant.
- **`hw_vram()`** via `nvidia-smi` — real GPU memory, not guessed.
- Verdict ladder `PASS / WARN / OOM`, ctx fallback, FreeToken RAM-spill mode.
- `DONE` — this is the core.

### B2. Live throughput measurement
- `/metrics` probe → median generation tok/s, stored in `cyberdeck.db`.
- `deck bench record` / `list`; CONSOLE **BENCH** writes the same table.
- `PARTIAL → EXTEND`: needs multi-trial medians, P50/P90, and per-loadout history.

### B3. Headless loadout autotune
- The plan from the session: sweep ctx/kv/ngl/ubatch on the **test port**,
  score by objective (tok/s vs reasoning), never touch the live service until
  `APPLY`. Runs headlessly and reports candidates. (`open` — the agent harness
  can scaffold the Rust command + HUD wiring.)

### B4. Test harness with service isolation
- Live unit stopped → draft spawned on test port (`LlamaCpp 18999`,
  `FreeToken 18998`) → OOM-scanned → `health_ok_any` polled → live restored.
- `DONE` — a real differentiator; no other tool does safe in-place loadout swaps.

### B5. Managed client rewiring
- `--managed` repoints dsh + opencode at the applied engine's port, with `.bak`
  discipline. Odysseus never touches your client stack.
- `DONE`.

### B6. GGUF remote fit in the browse flow
- Search/browse HF → background-prefetch fits for top results → verdict + VRAM
  column, per-quant. `DONE`.

---

## Parity gap matrix (build order)

Ordered by (a) relevance to cyberdeck as a **chat-for-everything workspace +
benchmark control room**, then (b) effort. Chat-surface completeness first, so
the app feels whole to use, then benchmark depth.

| # | Gap | Tracks | cyberdeck target | Effort |
|---|-----|--------|------------------|--------|
| 0 | **BringUp: one-click load** | Flagship flow | `deck bringup --model --engine` → derive max-ctx profile → verify on test port → install → bench & record. HUD `LOAD` button | M |
| 1 | **Multi-model residency (PORT MAP)** | Direction §1 | each engine a fixed port slot (:18000/:1919/:11434) with an optional bound profile; `deck use` = bind to slot *and start*, N residents coexist; keep single-swap as default | M |
| 2 | **Concurrent chat across residents** | Direction §2 | generalize HUD/CONSOLE: per-session engine+model pin, token streaming (not just opencode lines), live retarget of next message | M |
| 3 | **Bench-aware chat header** | Direction §3 | for each resident show tok/s + fit in chat header, so you can see where to type | S |
| 4 | **Compare / A·B bench** | Odysseus §4 | `deck bench compare` CLI + tab; blind-random same prompts across residents, tok/s + score, agent synthesis | M |
| 5 | **Autotune headless** | Advantage B3 | sweep ctx/kv/ngl/ubatch on test port, score by objective, `APPLY` best — feeds the rec header | M |
| 6 | Deep-research skill | §3 | agent skill: search→read→synthesize→report | S |
| 7 | MCP picker per session | §1 | expose opencode MCP servers in agent panel | S |
| 8 | File attach to prompt | §1 | pass attach path into `opencode run` / chat | S |
| 9 | Cookbook model suggestion | §2 | rank MARKET/BROWSE by `fit(ngl)` → "best that PASSes" | S |
| 10 | Provider picker (Ollama/API) | §1 | generic OpenAI-compatible provider alongside llamacpp/FreeToken | S |
| 11 | Skills surfacing | §1 | list `~/.config/opencode/skills` in agent panel | XS |
| 12 | Scheduled bench/watch | §7 sliver | cron-style auto `deck bench` + SIGNALS on interval | S |

Effort: `XS` <30 min · `S` <1 day · `M` 2–4 days.

---

## Explicitly out of scope (`SKIP`) — for now

Default-off, revisit only once the chat-for-everything core (#1–#3) is solid:
- Documents editor (§5), Email (§6), full Notes/Tasks/Calendar (§7), gallery /
  image editor / upload galleries, 2FA / public multi-user hosting.

Rationale: these are *productivity-app* features, not what makes cyberdeck
distinct. The order above puts real chat + real benchmarks first; if you later
want email triage or a notes pane, you can add them on top — this doc marks
them `SKIP` **today so they don't crowd the build**, not forever. Revisit each
once `chat for everything` is actually true.

---

## Definition of "parity reached"

cyberdeck is at-parity when:

1. **#0 lands:** `deck bringup` turns "model + engine" into a working,
   max-ctx, benchmarked loadout without touching flags.
2. **#1–#3 land:** multiple engines resident on fixed ports, concurrent chat
   sessions pinned to different models you can flip between, and each resident's
   bench/fit shown in the chat header.
3. `deck bench compare` produces a blind-scored, synthesized comparison.
4. The headless `autotune` loop picks and APPLY-able best config per objective,
   feeding the recommendations.
5. Agent + chat route to a live local engine by default, have MCP/attach/skills,
   and stream reliably (the event-wiring fix is in).

Update `## Status` header below as rows land.
