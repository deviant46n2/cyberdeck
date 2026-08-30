//! deck-feeds: HuggingFace watchlist poller, marketplace search, resumable
//! downloads, and new-release detection.
//!
//! Phase 4 (SIGNALS). Watches a set of orgs/users, fetches their most recent
//! models from the HF API, and reports only what hasn't been seen before —
//! filtered notifications, never a firehose. State (watchlist + seen ids) lives
//! in the shared cyberdeck SQLite index.
//!
//! Split across four focused modules:
//!   - `probe`    — curl transport + HF org polling + GGUF header Range fetch
//!   - `market`   — full-text search + repo file listing with HEAD-probed sizes
//!   - `download` — resumable `.part` streaming with cooperative cancel + shard sets
//!   - `watchlist`— watched-org state and new-model detection
//!   - `ollama`   — local ollama model enumeration
//!
//! Note on transport: ureq's connect/TLS path stalls indefinitely on some Linux
//! setups (IPv6-family dead ends that never trip its agent timer) where curl's
//! happy-eyeballs succeeds instantly. All remote I/O here shells out to the
//! system curl binary instead — present and fast on target machines, no extra
//! dependency.

mod download;
pub mod feeds;
mod market;
mod ollama;
mod probe;
mod watchlist;

pub use download::{Cancel, download_file_progress, shard_set_of};
pub use market::{
    MarketFile, SearchHit, model_files, parse_search, parse_siblings, remote_file_size,
    search_models,
};
pub use ollama::{OllamaModelInfo, ollama_models};
pub use probe::{HfModel, diff_new, fetch_gguf_bytes, fetch_org, parse_models};
pub use watchlist::{
    add_org, check, default_watchlist, ensure_seeds, list_watchlist, open, remove_org,
};
pub use feeds::{GithubSource, HfSource, Source, poll as feeds_poll};
