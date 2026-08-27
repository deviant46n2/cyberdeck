//! deck-feeds: HuggingFace watchlist poller and new-release detection.
//!
//! Phase 4 (SIGNALS). Watches a set of orgs/users, fetches their most recent
//! models from the HF API, and reports only what hasn't been seen before —
//! filtered notifications, never a firehose. State (watchlist + seen ids) lives
//! in the shared cyberdeck SQLite index.

use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;

// ---------------------------------------------------------- curl transport
//
// ureq's connect/TLS path stalls indefinitely on some Linux setups (IPv6-family
// dead ends that never trip its agent timer) where curl's happy-eyeballs
// succeeds instantly. All remote I/O here shells out to the system curl binary
// instead — present and fast on target machines, no extra dependency.

/// GET a URL as UTF-8 text (JSON APIs), failing loudly on non-2xx.
fn fetch_url(url: &str, timeout_secs: u64) -> Result<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            "--fail",
            "--show-error",
            "--max-time",
            &timeout_secs.to_string(),
            url,
        ])
        .output()
        .context("failed to spawn curl (is it installed?)")?;
    if !out.status.success() {
        anyhow::bail!(
            "GET {url} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn simple_encode(s: &str) -> String {
    s.replace(' ', "%20").replace('#', "%23")
}

#[derive(Debug, Clone)]
pub struct HfModel {
    pub id: String,
    pub author: String,
    pub created_at: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct HfRow {
    id: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Parse the HF `/api/models` JSON array into our model shape (pure, testable).
pub fn parse_models(json: &str) -> Result<Vec<HfModel>> {
    let rows: Vec<HfRow> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .map(|r| HfModel {
            author: r
                .author
                .unwrap_or_else(|| r.id.split('/').next().unwrap_or("").to_string()),
            created_at: r.created_at.unwrap_or_default(),
            id: r.id,
            downloads: r.downloads,
            likes: r.likes,
            pipeline_tag: r.pipeline_tag,
            tags: r.tags,
        })
        .collect())
}

/// Keeps only models whose id is not in `seen` (pure, testable).
pub fn diff_new(all: &[HfModel], seen: &[String]) -> Vec<HfModel> {
    let seen_set: std::collections::HashSet<&str> = seen.iter().map(|s| s.as_str()).collect();
    all.iter()
        .filter(|m| !seen_set.contains(m.id.as_str()))
        .cloned()
        .collect()
}

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// Fetch the most recent `limit` models authored by `org` from the HF API.
pub fn fetch_org(org: &str, limit: usize) -> Result<Vec<HfModel>> {
    let url = format!(
        "https://huggingface.co/api/models?author={}&sort=createdAt&direction=-1&limit={}",
        simple_encode(org),
        limit
    );
    let body = fetch_url(&url, 20)
        .with_context(|| format!("HF API request for '{org}' failed (offline?)"))?;
    parse_models(&body)
}

pub fn open() -> Result<rusqlite::Connection> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS watchlist (org TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY, org TEXT, seen_at TEXT);",
    )?;
    Ok(conn)
}

/// Default orgs to watch, per the cyberdeck lore.
pub fn default_watchlist() -> Vec<String> {
    vec![
        "unsloth".into(),
        "bartowski".into(),
        "ggml-org".into(),
        "nvidia".into(),
    ]
}

pub fn ensure_seeds(conn: &rusqlite::Connection) -> Result<()> {
    for org in default_watchlist() {
        conn.execute("INSERT OR IGNORE INTO watchlist (org) VALUES (?1)", [org])?;
    }
    Ok(())
}

pub fn list_watchlist(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT org FROM watchlist ORDER BY org")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.flatten().collect())
}

pub fn add_org(conn: &rusqlite::Connection, org: &str) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO watchlist (org) VALUES (?1)", [org])?;
    Ok(())
}

pub fn remove_org(conn: &rusqlite::Connection, org: &str) -> Result<()> {
    conn.execute("DELETE FROM watchlist WHERE org = ?1", [org])?;
    Ok(())
}

/// Poll every watched org; return only models not seen before, recording them.
pub fn check(conn: &rusqlite::Connection, limit: usize) -> Result<Vec<HfModel>> {
    let orgs = list_watchlist(conn)?;
    let mut news = Vec::new();
    for org in &orgs {
        let models = fetch_org(org, limit)?;
        for m in models {
            let seen = conn
                .query_row("SELECT 1 FROM seen WHERE id = ?1", [&m.id], |_| Ok(()))
                .is_ok();
            if !seen {
                conn.execute(
                    "INSERT OR IGNORE INTO seen (id, org, seen_at) VALUES (?1, ?2, ?3)",
                    [&m.id, org, &now()],
                )?;
                news.push(m);
            }
        }
    }
    Ok(news)
}

// ---------------------------------------------------------------- MARKET

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
struct SearchRow {
    id: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
}

/// Parse the HF `/api/models?search=` JSON array (pure, testable).
pub fn parse_search(json: &str) -> Result<Vec<SearchHit>> {
    let rows: Vec<SearchRow> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .map(|r| SearchHit {
            id: r.id,
            downloads: r.downloads,
            likes: r.likes,
            pipeline_tag: r.pipeline_tag,
            tags: r.tags,
            created_at: r.created_at.unwrap_or_default(),
        })
        .collect())
}

pub fn search_models(query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&sort=downloads&direction=-1&limit={}",
        simple_encode(query),
        limit
    );
    let body = fetch_url(&url, 20)
        .with_context(|| format!("HF search for '{query}' failed (offline?)"))?;
    parse_search(&body)
}

#[derive(Debug, Clone)]
pub struct MarketFile {
    pub rfilename: String,
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct ModelDetail {
    #[serde(default)]
    siblings: Vec<Sibling>,
}

#[derive(Deserialize)]
struct Sibling {
    rfilename: String,
}

/// Parse a single-model detail JSON into its sibling filenames (pure).
pub fn parse_siblings(json: &str) -> Result<Vec<String>> {
    let d: ModelDetail = serde_json::from_str(json)?;
    Ok(d.siblings.into_iter().map(|s| s.rfilename).collect())
}

/// Size of a remote file via a HEAD-style probe (follows redirects, reads the
/// final `content-length` header). `None` when the server hides it or we're
/// offline.
fn head_size(url: &str) -> Option<u64> {
    let out = std::process::Command::new("curl")
        .args(["-sIL", "--max-time", "20", url])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.to_lowercase().starts_with("content-length:"))
        .last()
        .and_then(|l| l.split(':').nth(1)?.trim().parse().ok())
}

/// List downloadable model files in a repo, resolving each file's size via a
/// HEAD request (probed 8-at-a-time so a 30-shard repo costs ~4 rounds of
/// latency instead of 30 sequential ones). Covers GGUF *and* safetensors
/// repos; other assets are skipped.
pub fn model_files(repo_id: &str) -> Result<Vec<MarketFile>> {
    let url = format!("https://huggingface.co/api/models/{repo_id}");
    let body = fetch_url(&url, 20)
        .with_context(|| format!("HF model lookup for '{repo_id}' failed (offline?)"))?;
    let names = parse_siblings(&body)?;

    let targets: Vec<String> = names
        .into_iter()
        .filter(|n| {
            let lower = n.to_lowercase();
            lower.ends_with(".gguf") || lower.ends_with(".safetensors")
        })
        .collect();

    let urls: Vec<String> = targets
        .iter()
        .map(|n| format!("https://huggingface.co/{repo_id}/resolve/main/{n}"))
        .collect();

    const PROBE_CONCURRENCY: usize = 8;
    let mut sizes: Vec<Option<u64>> = vec![None; urls.len()];
    std::thread::scope(|s| {
        for (slots, chunk_urls) in sizes
            .chunks_mut(PROBE_CONCURRENCY)
            .zip(urls.chunks(PROBE_CONCURRENCY))
        {
            s.spawn(move || {
                for (slot, u) in slots.iter_mut().zip(chunk_urls.iter()) {
                    *slot = head_size(u);
                }
            });
        }
    });

    Ok(targets
        .into_iter()
        .zip(sizes)
        .map(|(rfilename, size)| MarketFile { rfilename, size })
        .collect())
}

/// Resolve a repo file's size via HEAD without downloading it.
/// `None` when the server hides Content-Length.
pub fn remote_file_size(repo_id: &str, rfilename: &str) -> Option<u64> {
    head_size(&format!(
        "https://huggingface.co/{repo_id}/resolve/main/{name}",
        repo_id = repo_id,
        name = simple_encode(rfilename)
    ))
}

/// Cooperative cancellation handle shared between the caller and the
/// download stream. Cheap to clone via `Arc`.
#[derive(Debug, Default)]
pub struct Cancel(std::sync::atomic::AtomicBool);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    /// Request cancellation; the stream aborts at its next chunk boundary
    /// and the partial `.part` file is removed.
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

const DL_CHUNK: usize = 256 * 1024;
const CANCELLED_MSG: &str = "cancelled";

/// Stream a single repo file to `dest_dir`, calling `progress(downloaded, total)`
/// after every chunk (`total` is 0 when Content-Length is unavailable).
///
/// `expected_total` comes from a prior size probe; when present, a short
/// transfer fails loudly instead of silently landing a truncated model.
///
/// Writes to `<name>.part` and renames only on success so an interrupted or
/// cancelled transfer never leaves a partial model where the scanner would
/// index it. Returns Err("cancelled") when the cancel flag trips mid-stream.
pub fn download_file_progress(
    repo_id: &str,
    rfilename: &str,
    dest_dir: &std::path::Path,
    expected_total: Option<u64>,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &Cancel,
) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let url = format!(
        "https://huggingface.co/{repo_id}/resolve/main/{name}",
        repo_id = repo_id,
        name = simple_encode(rfilename)
    );

    let name = std::path::Path::new(rfilename)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| rfilename.to_string());
    let dest = dest_dir.join(&name);
    let part = dest_dir.join(format!("{name}.part"));

    // Stream via curl stdout. No --max-time: transfers run minutes-to-hours;
    // cancellation and pipeline errors are handled below.
    let mut child = std::process::Command::new("curl")
        .args(["-sSL", "--fail", "--show-error", &url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn curl for '{repo_id}/{rfilename}'"))?;
    let mut stderr_pipe = child.stderr.take();
    let mut reader = child.stdout.take().context("curl stdout unavailable")?;

    // Drain stderr on a side thread so curl can't block writing an error to it.
    let stderr_drain = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stderr_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut f = std::fs::File::create(&part).with_context(|| format!("create {part:?} failed"))?;
    let cleanup_part = |part: &std::path::Path| {
        let _ = std::fs::remove_file(part);
    };

    let mut buf = vec![0u8; DL_CHUNK];
    let mut done: u64 = 0;
    loop {
        if cancel.cancelled() {
            drop(f);
            let _ = child.kill();
            let _ = child.wait();
            cleanup_part(&part);
            anyhow::bail!(CANCELLED_MSG);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, expected_total.unwrap_or(0));
    }
    f.sync_all()?;
    drop(f);

    let status = child.wait()?;
    if !status.success() {
        cleanup_part(&part);
        anyhow::bail!(
            "download of '{repo_id}/{rfilename}' failed: {}",
            stderr_drain.join().unwrap_or_default().trim()
        );
    }

    if let Some(t) = expected_total
        && done != t
    {
        cleanup_part(&part);
        anyhow::bail!("download truncated: got {done} of {t} bytes from '{repo_id}/{rfilename}'");
    }

    std::fs::rename(&part, &dest).with_context(|| format!("finalize {dest:?} failed"))?;
    Ok(dest)
}

/// Group sibling filenames into ordered shard sets. Given any member of a
/// split archive (`-NNNNN-of-MMMMM.gguf` / `-NNNNN-of-MMMMM.safetensors`),
/// returns every shard of that same set in ascending part order. Single files
/// (no shard suffix) return themselves.
pub fn shard_set_of(chosen: &str, all: &[String]) -> Vec<String> {
    fn split_shard(name: &str) -> Option<(String, u32, u32)> {
        // Right-to-left: <prefix>-<NNNNN>-of-<MMMMM>.gguf|.safetensors
        let dot = name.rfind('.')?;
        let ext = &name[dot + 1..];
        if !(ext.eq_ignore_ascii_case("gguf") || ext.eq_ignore_ascii_case("safetensors")) {
            return None;
        }
        let base = &name[..dot];
        let five_digits = |s: &str| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit());

        // 1. shard count after the final dash
        let dash_total = base.rfind('-')?;
        let total_str = &base[dash_total + 1..];
        if !five_digits(total_str) {
            return None;
        }
        // 2. the literal "-of" marker before it
        let rest = base[..dash_total].strip_suffix("-of")?;
        // 3. part number before THAT dash
        let dash_part = rest.rfind('-')?;
        let part_str = &rest[dash_part + 1..];
        if !five_digits(part_str) {
            return None;
        }
        let prefix = &rest[..dash_part];
        if prefix.is_empty() {
            return None;
        }
        Some((
            prefix.to_string(),
            part_str.parse().ok()?,
            total_str.parse().ok()?,
        ))
    }

    let (prefix, _part, declared_total) = match split_shard(chosen) {
        Some(x) => x,
        None => return vec![chosen.to_string()],
    };
    let mut parts: Vec<(u32, String)> = all
        .iter()
        .filter_map(|n| {
            split_shard(n).and_then(|(p, idx, tot)| {
                (p == prefix && tot == declared_total).then_some((idx, n.clone()))
            })
        })
        .collect();
    parts.sort_by_key(|(idx, _)| *idx);
    // Keep full sets intact (all M parts present); otherwise fall back to just
    // the chosen file rather than queueing a set we can't open anyway.
    if parts.len() == declared_total as usize {
        parts.into_iter().map(|(_, n)| n).collect()
    } else {
        vec![chosen.to_string()]
    }
}

/// Fetch the first `max_bytes` of a GGUF file via HTTP Range, returning the
/// bytes and the total file size (from Content-Range). Used to parse GGUF
/// header metadata for fit estimation without downloading the full file.
///
/// `max_bytes` defaults to 2 MiB which comfortably covers all scalar KVs
/// (arch, block_count, embedding_length, file_type) before large tokenizer
/// arrays that the parser gracefully truncates past.
pub fn fetch_gguf_bytes(repo_id: &str, rfilename: &str, max_bytes: u64) -> Result<(Vec<u8>, u64)> {
    let url = format!(
        "https://huggingface.co/{repo_id}/resolve/main/{name}",
        repo_id = repo_id,
        name = simple_encode(rfilename)
    );
    let range = format!("bytes=0-{}", max_bytes.saturating_sub(1));

    // Dump response headers to a temp file (-D) while streaming the body on
    // stdout, so we can read Content-Range/Content-Length after the fact.
    let hdr_path = std::env::temp_dir().join(format!(
        "deck-hdr-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            "--fail",
            "--show-error",
            "--max-time",
            "30",
            "-H",
            &range,
            "-D",
            hdr_path.to_str().context("temp dir not utf-8")?,
            &url,
        ])
        .output()
        .with_context(|| format!("GGUF header fetch for '{repo_id}/{rfilename}' failed"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&hdr_path);
        anyhow::bail!(
            "GGUF header fetch for '{repo_id}/{rfilename}' failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let hdr_text = String::from_utf8_lossy(&std::fs::read(&hdr_path)?).into_owned();
    let _ = std::fs::remove_file(&hdr_path);

    // Total size from the FINAL Content-Range ("bytes 0-N/TOTAL") of a 206, or
    // fall back to final Content-Length / our cap.
    let total_size = hdr_text
        .lines()
        .filter(|l| l.to_lowercase().starts_with("content-range:"))
        .last()
        .and_then(|l| l.rsplit('/').next()?.trim().parse::<u64>().ok())
        .or_else(|| {
            hdr_text
                .lines()
                .filter(|l| l.to_lowercase().starts_with("content-length:"))
                .last()
                .and_then(|l| l.split(':').nth(1)?.trim().parse::<u64>().ok())
        })
        .unwrap_or(max_bytes);

    let mut buf: Vec<u8> = Vec::with_capacity(out.stdout.len());
    buf.extend_from_slice(&out.stdout[..out.stdout.len().min(max_bytes as usize)]);
    Ok((buf, total_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"id":"unsloth/GLM-5.3-Flash-GGUF","author":"unsloth","createdAt":"2026-08-26T14:00:00Z","downloads":0,"likes":132,"pipeline_tag":"text-generation","tags":["transformers","gguf"]},
      {"id":"unsloth/Qwen3.8-GGUF","author":"unsloth","createdAt":"2026-08-20T10:00:00Z","downloads":40,"likes":12,"tags":["gguf"]}
    ]"#;

    #[test]
    fn parses_hf_shape() {
        let models = parse_models(SAMPLE).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "unsloth/GLM-5.3-Flash-GGUF");
        assert_eq!(models[0].author, "unsloth");
        assert_eq!(models[0].likes, 132);
        assert_eq!(models[0].pipeline_tag.as_deref(), Some("text-generation"));
    }

    #[test]
    fn diff_returns_only_unseen() {
        let models = parse_models(SAMPLE).unwrap();
        let seen = vec!["unsloth/Qwen3.8-GGUF".to_string()];
        let fresh = diff_new(&models, &seen);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, "unsloth/GLM-5.3-Flash-GGUF");
    }

    const SEARCH: &str = r#"[
      {"id":"unsloth/Qwen3.8-27B-GGUF","likes":3010,"downloads":7638591,"pipeline_tag":"text-generation","tags":["gguf","qwen3_5"],"createdAt":"2026-08-13T08:28:40Z"},
      {"id":"Qwen/Qwen3.8-27B-FP8","likes":705,"downloads":3797538,"tags":["transformers"]}
    ]"#;

    #[test]
    fn parses_search() {
        let hits = parse_search(SEARCH).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "unsloth/Qwen3.8-27B-GGUF");
        assert_eq!(hits[0].downloads, 7_638_591);
        assert_eq!(hits[0].pipeline_tag.as_deref(), Some("text-generation"));
        assert!(hits[1].pipeline_tag.is_none());
    }

    const DETAIL: &str = r#"{"id":"x/Y-GGUF","siblings":[
      {"rfilename":"README.md"},
      {"rfilename":"UD-IQ1_M/Y-UD-IQ1_M-00001-of-00003.gguf","size":123},
      {"rfilename":"UD-IQ1_M/Y-UD-IQ1_M-00002-of-00003.gguf"}
    ]}"#;

    #[test]
    fn parses_siblings_and_filters_gguf() {
        let names = parse_siblings(DETAIL).unwrap();
        let gguf: Vec<_> = names
            .iter()
            .filter(|n| n.to_lowercase().ends_with(".gguf"))
            .cloned()
            .collect();
        assert_eq!(gguf.len(), 2);
        assert!(gguf[0].contains("00001"));
    }

    #[test]
    fn shard_set_groups_and_orders_parts() {
        let all = vec![
            "README.md".to_string(),
            "Y-UD-IQ1_M-00003-of-00003.gguf".to_string(),
            "Y-UD-IQ1_M-00001-of-00003.gguf".to_string(),
            "Y-UD-IQ1_M-00002-of-00003.gguf".to_string(),
            "Y-Q4_K_M.gguf".to_string(),
        ];
        // Clicking any member yields the full ordered set.
        assert_eq!(
            shard_set_of("Y-UD-IQ1_M-00002-of-00003.gguf", &all),
            vec![
                "Y-UD-IQ1_M-00001-of-00003.gguf",
                "Y-UD-IQ1_M-00002-of-00003.gguf",
                "Y-UD-IQ1_M-00003-of-00003.gguf",
            ]
        );
        // Single files pass through untouched.
        assert_eq!(shard_set_of("Y-Q4_K_M.gguf", &all), vec!["Y-Q4_K_M.gguf"]);
        // A set with a missing shard is NOT queueable — fall back to the file itself.
        let partial = vec![
            "S-00001-of-00004.safetensors".to_string(),
            "S-00002-of-00004.safetensors".to_string(),
            "S-00004-of-00004.safetensors".to_string(),
        ];
        assert_eq!(
            shard_set_of("S-00002-of-00004.safetensors", &partial),
            vec!["S-00002-of-00004.safetensors"]
        );
    }

    #[test]
    fn cancel_flag_is_cooperative() {
        let c = Cancel::new();
        assert!(!c.cancelled());
        c.cancel();
        assert!(c.cancelled());
    }

    /// Real network download of a tiny GGUF (~1.1 MiB) proving streaming,
    /// progress callbacks, and `.part` rename end-to-end. Ignored by default
    /// so offline runs stay green:
    ///   cargo test -p deck-feeds -- --ignored --nocapture
    #[test]
    #[ignore]
    fn downloads_real_tiny_gguf_when_online() {
        let dest = std::env::temp_dir().join("deck-dl-test-models");
        let cancel = Cancel::new();
        let mut last_seen: u64 = 0;
        let mut total_reported: u64 = 0;
        let mut progress = |done: u64, total: u64| {
            assert!(done >= last_seen, "progress must be monotonic");
            assert_ne!(done, 0, "first chunk must land");
            if total > 0 {
                total_reported = total;
            }
            last_seen = done;
        };
        let path = download_file_progress(
            "ggml-org/models",
            "tinyllamas/stories260K.gguf",
            &dest,
            Some(1_185_376),
            &mut progress,
            &cancel,
        )
        .expect("download succeeds when online");

        let bytes = std::fs::read(&path).expect("file exists at final path");
        assert_eq!(&bytes[..4], b"GGUF", "magic bytes intact after rename");
        assert!(
            total_reported > 0,
            "content-length surfaced through progress"
        );
        // no stray .part left behind
        assert!(
            !std::path::Path::new(&path.with_extension("part")).exists()
                && !path
                    .parent()
                    .unwrap()
                    .read_dir()
                    .unwrap()
                    .any(|e| { e.unwrap().file_name().to_string_lossy().contains(".part") }),
            ".part cleaned up"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(dest);
    }
}

// ----------------------------------------------------------- ollama integration

/// Scan ollama's installed models: query the local API to get model names,
/// then parse `ollama show --modelfile <name>` to find the `FROM` blob path.
/// Only returns models that have an on-disk GGUF file (no cloud models).
pub fn ollama_models() -> Result<Vec<OllamaModelInfo>> {
    let text = fetch_url("http://localhost:11434/api/tags", 5)?;

    #[derive(serde::Deserialize)]
    struct OllamaList {
        models: Vec<OllamaEntry>,
    }
    #[derive(serde::Deserialize)]
    struct OllamaEntry {
        name: String,
    }

    let list: OllamaList = serde_json::from_str(&text)?;
    let mut results = Vec::new();

    for entry in list.models {
        // Parse modelfile to find the FROM blob path.
        let output = std::process::Command::new("ollama")
            .args(["show", "--modelfile", &entry.name])
            .output();

        if let Ok(o) = output {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(path_str) = line.strip_prefix("FROM ") {
                    let path_str = path_str.trim();
                    if path_str.is_empty() {
                        continue;
                    }
                    let p = std::path::Path::new(path_str);
                    if p.is_file() {
                        if let Ok(meta) = p.metadata() {
                            results.push(OllamaModelInfo {
                                name: entry.name.clone(),
                                path: path_str.to_string(),
                                size: meta.len(),
                            });
                        }
                    }
                    break;
                }
            }
        }
    }

    Ok(results)
}

#[derive(Debug, Clone)]
pub struct OllamaModelInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
}
