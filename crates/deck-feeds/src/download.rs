//! Resumable download streaming to `<name>.part` with a cooperative cancel,
//! and multi-part GGUF/safetensors shard-set grouping.

use std::io::{Read, Write};

use anyhow::{Context, Result};

use crate::probe::simple_encode;

/// Cooperative cancellation handle shared between the caller and the
/// download stream. Cheap to clone via `Arc`.
#[derive(Debug, Default)]
pub struct Cancel(std::sync::atomic::AtomicBool);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    /// Request cancellation; the stream aborts at its next chunk boundary. The
    /// partial `.part` file is kept so the transfer can later resume from where
    /// it stopped — callers that no longer want the partial should remove it
    /// explicitly (see `deck_tauri::download_remove`).
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
/// index it. When a `<name>.part` already exists the transfer **resumes** from
/// its size (curl `-C -`) — the download manager's STOP / START. Returns
/// Err("cancelled") when the cancel flag trips mid-stream.
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

    // Resume from an existing partial if we have one. curl's `-C -` picks the
    // offset up from the local file automatically and issues an HTTP Range.
    let resume_from = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    // Stream via curl stdout. No --max-time: transfers run minutes-to-hours;
    // cancellation and pipeline errors are handled below.
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sSL", "--fail", "--show-error", &url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if resume_from > 0 {
        cmd.args(["-C", "-"]);
    }
    let mut child = cmd
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

    // Append so a resumed stream continues the partial instead of truncating.
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part)
        .with_context(|| format!("open {part:?} failed"))?;
    let cleanup_part = |part: &std::path::Path| {
        let _ = std::fs::remove_file(part);
    };

    let mut buf = vec![0u8; DL_CHUNK];
    let mut done: u64 = resume_from;
    progress(done, expected_total.unwrap_or(0));
    loop {
        if cancel.cancelled() {
            // User STOP: keep `.part` so START can resume from here.
            drop(f);
            let _ = child.kill();
            let _ = child.wait();
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Real network download of a tiny GGUF (~1.1 MiB): streaming, progress
    /// callbacks, and `.part` rename end-to-end. Ignored so offline stays green:
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
