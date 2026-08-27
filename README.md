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
| 4 | Signals | watchlists, engine staleness, filtered notifications |
| 5 | Market | discovery + downloads |
| 6 | FreeToken | full integration, bench history |

## Dev loop

```sh
cargo build                 # workspace
cargo test -p deck-core     # parser tests
cargo run -p deck-cli -- --help
```
