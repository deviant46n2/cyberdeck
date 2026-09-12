//! Runtime availability: which backends are actually installed.
//!
//! The pure resolvers (`find_bin`, `availability_for`, …) live in
//! `deck_core::runtime` so the fit planner and both doors share them without
//! spawning. This module adds the one operation that must spawn —
//! `probe_version` — which is strictly opt-in, because a manifest that points
//! at a daemon would otherwise start serving.

pub use deck_core::runtime::{
    RuntimeAvailability, availability_for, find_bin, is_installed, system_path_dirs,
};

use std::path::Path;

/// Run `<bin> --version` and return the first non-empty output line. Strictly
/// opt-in: only call on explicit user request, and never on a manifest whose
/// binary might be a daemon (it could start serving instead of printing). A
/// binary that ignores `--version` is killed after ~5s, never awaited forever.
pub fn probe_version(bin: &Path) -> Option<String> {
    let mut child = std::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let mut waited = 0u32;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if waited >= 50 {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                waited += 1;
            }
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_probe_reads_first_line_and_rejects_missing() {
        let base = std::env::temp_dir().join(format!("deck-avail-v-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let b = base.join("mockbin");
        std::fs::write(&b, b"#!/bin/sh\necho v1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&b).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&b, perms).unwrap();
        }
        assert_eq!(probe_version(&b).as_deref(), Some("v1"));
        assert!(probe_version(&base.join("missing")).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
