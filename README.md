# cyberdeck

Local LLM fleet manager. Track model releases, control what's downloaded, and
load models through llama.cpp or FreeToken with full visibility into context
size, VRAM fit, and live throughput — everything at a glance.

Named for what it is: a personal machine for loading capability shards.

## Stack

- `crates/deck-core` — GGUF/safetensors scanning, inventory index, VRAM fit estimator, shard-dedup detection
- `crates/deck-engines` — engine control (llama.cpp :18000, FreeToken :1919), systemd unit generation, managed client rewiring
- `crates/deck-feeds` — HF watchlist, engine staleness checks, market/discovery feed
- `crates/deck-cli` — the `deck` binary; every feature lands here first, headless-tested
- Tauri 2 + React frontend (phases 3+) — HUD / VAULT / SIGNALS / MARKET / LOADOUTS / CONSOLE

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

## Dev loop

```sh
cargo build                 # workspace (incl. Tauri app binary)
cargo test                  # parser + engine + importer + command-API tests
cargo run -p deck-cli -- --help

# desktop app (Tauri 2 + React)
cd frontend && npm install && npm run build   # bundle the UI
npm run tauri dev            # vite dev server + Rust app (needs a display)
# if npm blocked esbuild's postinstall: `npm rebuild esbuild`
```

## Layout

- `crates/deck-tauri` — serializable command API (the UI<->crate bridge, unit-tested)
- `src-tauri` — the Tauri 2 binary; thin `#[tauri::command]` wrappers over deck-tauri
- `frontend` — React + Vite UI (HUD / VAULT / SIGNALS / MARKET / LOADOUTS / CONSOLE)

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

deck use <name> [--dry-run]                # render+install unit (.bak first),
                                           # restart service, health-wait, ctx ladder
```

`deck use` preserves the alias+port contract so clients (opencode, dsh) keep
working. On a failed load it walks `ctx_size` → the profile's `ctx_ladder`,
then restores the last-good `.bak`. `--dry-run` prints the generated unit
without touching the live service.

