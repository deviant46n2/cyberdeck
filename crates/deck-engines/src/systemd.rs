//! systemd unit lifecycle: install with timestamped backups, daemon reload,
//! start/stop, and the ctx-ladder apply path with a last-good restore.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use deck_core::profile::{Engine, Profile};

use crate::health::health_wait;
use crate::unit::{generated_dir, render_unit, systemd_dir};

/// Copies an existing unit to `<unit>.bak.<timestamp>` before overwriting.
/// Returns the backup path, or None if there was nothing to back up.
pub fn backup_existing(unit_path: &Path) -> Result<Option<PathBuf>> {
    if !unit_path.exists() {
        return Ok(None);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let bak = unit_path.with_extension(format!("service.bak.{ts}"));
    std::fs::copy(unit_path, &bak)
        .with_context(|| format!("backing up {}", unit_path.display()))?;
    Ok(Some(bak))
}

/// Generic `.bak.<nanos>` backup that preserves the original file extension
/// (e.g. `settings.yaml` -> `settings.yaml.bak.<ts>`). Used for non-unit files
/// like client configs.
pub fn backup_file(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let bak = path.with_extension(format!("{ext}.bak.{ts}"));
    std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    Ok(Some(bak))
}

/// Restores the most recent `.bak` backup of a unit, if any exist.
pub fn restore_last_good(dir: &Path, unit_name: &str) -> Result<bool> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(unit_name) && n.contains(".bak."))
                .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    if let Some(latest) = candidates.last() {
        let target = dir.join(unit_name);
        std::fs::copy(latest, &target)?;
        eprintln!("restored last-good unit from {}", latest.display());
        reload_daemon()?;
        return Ok(true);
    }
    Ok(false)
}

/// Installs the rendered unit (and a LimitMEMLOCK drop-in for llama.cpp),
/// backing up any prior unit first. Returns the paths written.
pub fn install(p: &Profile, dry_run: bool) -> Result<Vec<PathBuf>> {
    let unit_name = p.engine.systemd_unit();
    let dir = systemd_dir();
    std::fs::create_dir_all(&dir)?;
    let gen_dir = generated_dir();
    std::fs::create_dir_all(&gen_dir)?;

    let unit_path = dir.join(unit_name);
    let content = render_unit(p);

    if dry_run {
        println!("--- {unit_name} (dry-run, not written) ---");
        println!("{content}");
        return Ok(vec![unit_path]);
    }

    if let Some(bak) = backup_existing(&unit_path)? {
        eprintln!("backed up prior unit -> {}", bak.display());
    }
    std::fs::write(&unit_path, &content)
        .with_context(|| format!("writing {}", unit_path.display()))?;

    let gen_dir = generated_dir();
    let gen_path = gen_dir.join(unit_name);
    std::fs::write(&gen_path, &content)?;

    if p.engine == Engine::LlamaCpp {
        let dropin_dir = dir.join(format!("{}.d", unit_name.trim_end_matches(".service")));
        std::fs::create_dir_all(&dropin_dir)?;
        std::fs::write(
            dropin_dir.join("memlock.conf"),
            "[Service]\nLimitMEMLOCK=infinity\n",
        )?;
    }

    reload_daemon()?;
    Ok(vec![unit_path, gen_path])
}

pub fn reload_daemon() -> Result<()> {
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .with_context(|| "systemctl daemon-reload")?;
    Ok(())
}

pub fn start(unit: &str) -> Result<()> {
    Command::new("systemctl")
        .args(["--user", "restart", unit])
        .status()
        .with_context(|| format!("systemctl restart {unit}"))?;
    Ok(())
}

pub fn stop(unit: &str) -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "stop", unit])
        .status();
    Ok(())
}

/// Start a system-level service (no `--user` flag). Used for engines like
/// ollama whose unit lives under `/usr/lib/systemd/system/`.
pub fn start_system(unit: &str) -> Result<()> {
    Command::new("systemctl")
        .args(["start", unit])
        .status()
        .with_context(|| format!("systemctl start {unit}"))?;
    Ok(())
}

/// Stop a system-level service (no `--user` flag).
pub fn stop_system(unit: &str) -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["stop", unit])
        .status();
    Ok(())
}

/// Check whether a systemd unit is currently active (running).
/// Works for both user and system services.
pub fn is_active(unit: &str, system: bool) -> bool {
    let mut cmd = Command::new("systemctl");
    if !system {
        cmd.arg("--user");
    }
    cmd.args(["is-active", "--quiet", unit]);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Applies a profile: renders + installs + starts + health-waits. On failure,
/// walks the ctx ladder (rewriting --ctx-size) and retries. Final fallback
/// restores the previously-active unit from its .bak if present.
pub fn apply(p: &Profile, dry_run: bool) -> Result<()> {
    let unit = p.engine.systemd_unit();
    install(p, dry_run)?;
    if dry_run {
        return Ok(());
    }
    start(unit)?;
    if health_wait(&p.host, p.port, Duration::from_secs(60)) {
        return Ok(());
    }
    eprintln!("health check failed for '{}', walking ctx ladder", p.name);
    for ctx in p.ctx_ladder.iter().copied() {
        let mut reduced = p.clone();
        reduced.ctx_size = ctx;
        eprintln!("retry with ctx-size={ctx}");
        install(&reduced, false)?;
        start(unit)?;
        if health_wait(&reduced.host, reduced.port, Duration::from_secs(60)) {
            return Ok(());
        }
    }
    eprintln!("all ctx steps failed; attempting last-good restore");
    let dir = systemd_dir();
    if restore_last_good(&dir, unit).unwrap_or(false) {
        let _ = start(unit);
    }
    Ok(())
}
