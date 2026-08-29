//! `deck download <repo>`: resolve a repo's .gguf files, pick one, and
//! stream it resumably into `~/models`. Single-file, blocking, progress
//! to stderr. The queue manager stays in the Tauri app; the CLI door
//! makes the first step of the loop reproducible headlessly.

use anyhow::Result;
use std::time::Instant;

use deck_core::store::models_dir;
use deck_feeds::{Cancel, download_file_progress, model_files, MarketFile};

/// Download a repo's model file into ~/models.
///
/// * `repo` — HuggingFace repo id (e.g. `unsloth/Qwen3.8-GGUF`).
/// * `file` — explicit filename (exact or suffix match).
/// * `quant` — pick the largest .gguf whose name contains this string.
/// * `dry_run` — resolve and print the pick without downloading.
pub fn run(repo: &str, file: Option<&str>, quant: Option<&str>, dry_run: bool) -> Result<()> {
    let files = model_files(repo)?;
    let ggufs: Vec<MarketFile> = files.iter().filter(|f| f.rfilename.ends_with(".gguf")).cloned().collect();
    if ggufs.is_empty() {
        anyhow::bail!("no .gguf files found in {repo}");
    }

    let chosen = pick(file, quant, &ggufs)
        .ok_or_else(|| anyhow::anyhow!("no matching .gguf in {repo}"))?;

    if dry_run {
        println!(
            "{}  {}  ({})",
            chosen.rfilename,
            chosen.size.map(|s| format!("{} bytes", s)).unwrap_or_else(|| "?".into()),
            repo,
        );
        return Ok(());
    }

    let dest = models_dir();
    let cancel = Cancel::new();
    let mut last_emit_done: u64 = 0;
    let mut last_emit_at = Instant::now();
    let mut progress = |done: u64, total: u64| {
        let due = done.saturating_sub(last_emit_done) >= 4_000_000
            || done == 0
            || last_emit_at.elapsed().as_millis() >= 400;
        if due {
            let pct = if total > 0 {
                (done as f64 / total as f64 * 100.0) as u64
            } else {
                0
            };
            eprintln!("[download] {}/{} ({:.0}%)", done, total, pct);
            last_emit_done = done;
            last_emit_at = Instant::now();
        }
    };
    eprintln!("Downloading {}/{} → {}", repo, chosen.rfilename, dest.display());
    let path = download_file_progress(
        repo,
        &chosen.rfilename,
        &dest,
        chosen.size,
        &mut progress,
        &cancel,
    )?;
    println!("Saved {}", path.display());
    Ok(())
}

/// Pick the file to download from a `.gguf` candidate list.
fn pick<'a>(file: Option<&'a str>, quant: Option<&'a str>, ggufs: &'a [MarketFile]) -> Option<&'a MarketFile> {
    let mut iter = ggufs.iter();
    match (file, quant) {
        (Some(f), _) => iter.find(|m| m.rfilename == f || m.rfilename.ends_with(f)),
        (None, Some(q)) => iter.filter(|m| m.rfilename.contains(q)).max_by_key(|m| m.size.unwrap_or(0)),
        (None, None) => iter.max_by_key(|m| m.size.unwrap_or(0)),
    }
}
