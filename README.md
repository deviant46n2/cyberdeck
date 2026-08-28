# cyberdeck

Local LLM fleet manager. Track model releases, control what's downloaded, and
load models through llama.cpp or FreeToken with full visibility into context
size, VRAM fit, and live throughput — everything at a glance.

Named for what it is: a personal machine for loading capability shards.

## Stack

- `crates/deck-core` — GGUF/safetensors scanning, inventory index, VRAM fit estimator, shard-dedup detection, GGUF header parser (EOF-tolerant for partial Range reads)
- `crates/deck-engines` — engine control (llama.cpp :18000, FreeToken :1919), systemd unit generation, managed client rewiring
- `crates/deck-feeds` — HF watchlist, engine staleness checks, market/discovery feed, GGUF header fetch for remote fit
- `crates/deck-cli` — the `deck` binary; every feature lands here first, headless-tested
- Tauri 2 + React frontend (phases 3+) — HUD / VAULT / SIGNALS / MARKET / BROWSE / LOADOUTS / CONSOLE

## Principles

1. **CLI-first.** The desktop app consumes crate APIs; it never gates them.
2. **The alias contract.** Swaps preserve `qwen3.8-27b @ :18000`; downstream clients are rewired automatically (with backups), never reconfigured by hand.
3. **Lore as data.** Hard-won VRAM facts (desktop reserve, ckb-next cost, ctx fallback ladder) live in config, not comments.
4. **Boring internals.** Neon synthwave shell, plain Rust underneath.
5. **Backups before writes.** Any file cyberdeck generates over gets a timestamped `.bak` first.

## Phases

| # | Name | Ships |
|---|------|-------|
| 1 | Vault | scanner, dedup report, fit estimator, wrapper-script importer |
| 2 | Loadouts | systemd takeover, one-command swaps, OOM ladder walk |
| 3 | HUD | Tauri app: live gauges, console pane, tray |
| 4 | Signals | HF watchlist + new-release detection (filtered, deduped) |
| 5 | Market | HF search + GGUF discovery + one-click download to ~/models |
| 6 | FreeToken | offload VRAM fit (RAM spill) + live bench history + engine status |
| 7 | Agent | CONSOLE runs `opencode run` as a streaming agent (live output) |
| 8 | Browse | browse HF sources with exact remote GGUF fit (header-fetch + nvidia-smi) |

## Dev loop

```sh
cargo build                 # workspace (incl. Tauri app binary)
cargo test                  # parser + engine + importer + command-API tests
cargo run -p deck-cli -- --help

# desktop app (Tauri 2 + React) — run `tauri` from the REPO ROOT, not frontend/
cd frontend && npm install && npm run build   # bundle the UI (or `npm run dev` for HMR)
cd .. && npm run tauri dev                    # vite dev server + Rust app (needs a display)
# if esbuild's postinstall step was blocked, rebuild it: `npm rebuild esbuild`
```

## Layout

- `crates/deck-tauri` — serializable command API (the UI<->crate bridge, unit-tested)
- `src-tauri` — the Tauri 2 binary; thin `#[tauri::command]` wrappers over deck-tauri
- `frontend` — React + Vite UI (HUD / VAULT / SIGNALS / MARKET / BROWSE / LOADOUTS / CONSOLE)

## CLI

```sh
deck scan                                  # refresh model index + report dupes
deck list [--json]                         # indexed models
deck fit --model <path> --ctx 32768 \      # VRAM fit verdict (PASS/WARN/OOM)
    [--kv-bytes 0.5] [--ngl 1.0] [--kv-layers N] [--reserve 1600]

deck profile import --engine llamacpp \    # seed a loadout from a launch script
    --script ~/.local/share/llama-server/run-llama-server.sh --name qwen
deck profile import --engine freetoken \   # FreeToken wrapper -> loadout
    --script ~/.local/share/freetoken/run-freetoken.sh --name qwen-ft
deck profile new --model <path> --engine llamacpp --name custom \   # from flags
    [--port 18000] [--alias qwen3.8-27b] [--ctx 32768] [--ngl 64] [--draft <gguf>]
deck profile list [--json]

deck use <name> [--dry-run] [--managed] [--resident]
                                            # render+install unit (.bak first),
                                            # restart service, health-wait, ctx ladder
deck engines status [--host 127.0.0.1]     # live PORT MAP per engine slot
deck engines stop <engine>                 # stop a slot, other residents stay up
deck engines list | bin <engine> [path]    # runtime registry / per-engine binary

deck bringup --model <path> --engine <llamacpp|freetoken> \
    [--name <loadout>] [--fast] [--dry-run]
```

`deck use` preserves the alias+port contract so clients (opencode, dsh) keep
working. On a failed load it walks `ctx_size` → the profile's `ctx_ladder`,
then restores the last-good `.bak`. `--dry-run` prints the generated unit
without touching the live service.

### Multi-model residency (PORT MAP)

Each engine owns a **fixed port slot** — llama.cpp `:18000`, FreeToken `:1919`,
Ollama `:11434` — with its own systemd unit, so all three can run at once.
`deck use --resident` binds a loadout to its engine's slot **and** runs it
alongside the other slots (plain `deck use` stays the one-at-a-time swap). The
binding is recorded in the `residents` table; `deck engines status` shows the
live map — bound profile, systemd/health state, resident flag — and
`deck engines stop <engine>` takes a single slot down without touching the
rest. The app's `port_map_status` surfaces the same map to the UI.

```sh
deck bench record [--engine llamacpp] [--host 127.0.0.1] [--port 18000] \
    [--model ?] [--ctx 0]        # probe /metrics, store generation tok/s
deck bench list                  # recent readings (shared with the app)
```

`deck bench` and the app's CONSOLE **BENCH** button write to the same
`cyberdeck.db` table, so a reading taken on either side shows up on both. Both
require the engine to be launched with `--metrics` (the generated loadout units
do this automatically; a hand-started server may not).

## BringUp (one-click load)

The flagship flow: **pick a model, pick an engine, let cyberdeck do the rest.**

`deck bringup --model <path> --engine freetoken`:

1. **Derives** the full loadout from the model's real header + detected VRAM
   (`hw_vram`) — the largest context window that still fits, KV cache type,
   layer offload (FreeToken spills weights to RAM), Flash Attention, reasoning
   budget for reasoning models, and engine-specific server flags. Handles both
   GGUF files and safetensors model-dirs (e.g. FreeToken NVFP4 shards).
2. **Verifies headlessly** on a dedicated test port (`:18999` llmacpp /
   `:18998` freetoken) — the live service is untouched. Watches for OOM,
   walks the ctx ladder down if the max-ctx candidate OOMs, and only proceeds
   once it actually serves.
3. **Installs + starts** the verified-good loadout, then **benches** it into
   `cyberdeck.db` so the chat header shows tok/s + fit.

`--fast` skips the test-port verification (apply directly). `--dry-run` derives
and prints without touching anything.

## Views (desktop app)

- **HUD** — models-on-disk, wasted-dupe GiB, live engine status dots for
  llamacpp `:18000` / freetoken `:1919`, and the **fit estimator** (context
  slider + a FreeToken-offload toggle that spills weights to RAM so NVFP4 models
  stop false-OOMing). One-click **APPLY** swaps the active loadout.
- **VAULT** — full inventory table; duplicate shards are flagged red.
- **SIGNALS** — HF org watchlist + "CHECK NOW" that surfaces new releases
  (deduped against what you've already seen).
- **MARKET** — search HuggingFace, expand a repo to list its GGUF files with
  real sizes (resolved via `HEAD`), and download straight to `~/models`.
- **BROWSE** — browse HF models from your watched orgs or by free-text search,
  expand to see available GGUF quant files with real sizes, and hit **FIT** to
  get an exact VRAM verdict — the app fetches the GGUF header via HTTP Range,
  parses `n_layers`/`n_embd`/quant from the metadata, and runs the fit estimator
  against detected GPU VRAM (nvidia-smi). Context slider applies to all fits.
- **LOADOUTS** — preview or apply a generated systemd unit (with confirm;
  always takes a `.bak` first).
- **CONSOLE** — engine telemetry + **BENCH tok/s** history, the last rendered
  unit, and the **agent panel**.

## Agentic CONSOLE

The CONSOLE agent panel runs `opencode run` as a real coding session: type a
task, pick a project dir, optionally pin a model, and hit **RUN AGENT**. Each
run opens its own session card with a live terminal pane — **multiple sessions
run concurrently**, so you can kick off several agents at once. **STOP** ends a
single session by id; finished cards can be dismissed.

- **`--auto` (the "danger" checkbox)** maps to opencode's `--auto`: it
  auto-approves permissions so the agent can modify files headlessly. Leave it
  **off** unless you intend the agent to write changes unprompted.
- Plain (non-TTY) output is streamed, so you see the agent's actual text, not a
  TUI.

## MANAGED rewiring

By default `deck use` is **Advisory**: it preserves the alias+port contract, so
clients like opencode/dsh keep pointing where they were. Pass `--managed`
(CLI) or flip the managed toggle (app LOADOUTS/use) to also **repoint dsh and
opencode at the applied engine's port** — so the rest of your stack follows the
swap. Every config file touched gets a timestamped `.bak` first
(`settings.yaml.bak.<ns>`, `opencode.json.bak.<ns>`), mirroring the unit
discipline. This is opt-in precisely because it rewrites files outside
cyberdeck's own control.

