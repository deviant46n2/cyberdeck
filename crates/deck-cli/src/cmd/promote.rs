//! `deck promote` — stage a release build into a separate "production"
//! install that your real workloads use, while you keep hacking with
//! `cargo tauri dev`.
//!
//! The promote model: the repo tree is the dev workspace; the promoted
//! binary lives in `$CYBERDECK_PROD_ROOT` (default `~/.local/share/
//! cyberdeck-prod`) and is launched through a wrapper at `~/.local/bin/
//! cyberdeck` that points `XDG_CONFIG_HOME`/`XDG_DATA_HOME` at a dedicated
//! `cyberdeck-prod` state tree — so the prod instance never shares its DB,
//! generated units, or settings with the dev tree.
//!
//! The engine PORT/ALIAS contract (AGENTS.md §4) is unchanged: both dev and
//! prod render systemd units for the same fixed ports (:18000/:1919/:11434),
//! so promote the prod instance and dev should NOT own the engine slots at
//! the same time. State is cleanly separated; the ports are the one shared
//! thing.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Build + stage target binary names. The app is the GUI you pilot workloads
/// with; `deck` (this CLI) is kept in sync so the terminal door matches.
const TARGETS: &[(&str, &str)] = &[("cyberdeck", "cyberdeck"), ("deck", "deck")];

/// Resolve the prod install root (env override, else `~/.local/share/cyberdeck-prod`).
fn prod_root() -> PathBuf {
    if let Some(r) = std::env::var_os("CYBERDECK_PROD_ROOT") {
        return PathBuf::from(r);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/cyberdeck-prod")
}

fn repo_root() -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("git rev-parse failed — are you in the cyberdeck repo?")?;
    if !out.status.success() {
        anyhow::bail!("not inside a git work tree (cyberdeck repo root)");
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// `~/.local/bin/<name>` — where prod door wrappers are installed.
fn home_bin(name: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/bin")
        .join(name)
}

fn build_release() -> Result<()> {
    println!("building release binaries…");
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "cyberdeck", "-p", "deck-cli"])
        .status()
        .context("failed to run cargo build --release")?;
    if !status.success() {
        anyhow::bail!("cargo build --release failed (exit {status})");
    }
    Ok(())
}

fn git_state() -> String {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "?".into());
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty {
        format!("{sha} (dirty work tree)")
    } else {
        format!("{sha} (clean)")
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).ok();
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Write wrapper shell scripts that run the installed binaries with the prod
/// XDG state tree, so BOTH doors (GUI app + deck CLI) are isolated from the
/// dev tree's DB/settings. Returns the app wrapper path.
fn write_wrappers(install: &Path) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let bin_dir = home.join(".local/bin");
    std::fs::create_dir_all(&bin_dir)
        .with_context(|| format!("creating {}", bin_dir.display()))?;
    // Dedicated config/data trees so the prod instance is fully isolated
    // from the dev-tree state (~/.config/cyberdeck + ~/.local/share/cyberdeck).
    let cfg = home.join(".config/cyberdeck-prod");
    let data = home.join(".local/share/cyberdeck-prod/data");

    let mut app_wrapper = None;
    for (name, src) in [("cyberdeck", "cyberdeck"), ("deck", "deck")] {
        let wrapper = bin_dir.join(name);
        let bin = install.join(src);
        let script = format!(
            "#!/usr/bin/env bash\n\
             # cyberdeck prod wrapper (written by `deck promote`) — do not edit.\n\
             exec env XDG_CONFIG_HOME={cfg} XDG_DATA_HOME={data} {bin} \"$@\"\n",
            cfg = cfg.display(),
            data = data.display(),
            bin = bin.display(),
        );
        std::fs::write(&wrapper, script)
            .with_context(|| format!("writing {}", wrapper.display()))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
            .context("chmod +x wrapper")?;
        if name == "cyberdeck" {
            app_wrapper = Some(wrapper);
        }
    }
    Ok(app_wrapper.expect("cyberdeck wrapper always requested"))
}

/// `deck promote` — build, ask, install, and report.
pub fn run() -> Result<()> {
    let repo = repo_root()?;
    let install = prod_root();

    println!("repo:      {}", repo.display());
    println!("install:   {}", install.display());
    println!("git:       {}", git_state());

    // Build first so the user only answers the gate once, after a successful
    // build (a stale/failed build should not prompt).
    build_release()?;

    if !confirm("promote this build to production?") {
        println!("promotion cancelled — nothing changed.");
        return Ok(());
    }

    // Copy the freshly built binaries into the install dir.
    std::fs::create_dir_all(&install)
        .with_context(|| format!("creating {}", install.display()))?;
    for (target, name) in TARGETS {
        let src = repo.join(format!("target/release/{target}"));
        let dst = install.join(name);
        if !src.exists() {
            println!("warning: {} missing (expected at {}), skipping", target, src.display());
            continue;
        }
        std::fs::copy(&src, &dst)
            .with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
        println!("installed {} -> {}", target, dst.display());
    }

    // (Re)write the shortcuts; both doors now run the prod binaries.
    let wrapper = write_wrappers(&install)?;
    println!("shortcut:  {} -> prod cyberdeck", wrapper.display());
    println!("shortcut:  {} -> prod deck", home_bin("deck").display());

    // Surface the "as well as master" half of the ask without force-pushing.
    println!();
    println!("promotion complete.");
    println!("run the app:  {}", wrapper.display());
    println!("run the cli:  {}", home_bin("deck").display());
    println!("state lives:  ~/.config/cyberdeck-prod + ~/.local/share/cyberdeck-prod/data");
    if confirm("also push master to origin?") {
        let status = Command::new("git")
            .args(["push", "origin", "master"])
            .status()
            .context("git push origin master failed")?;
        if !status.success() {
            anyhow::bail!("git push failed — nothing was lost, resolve and re-run");
        }
        println!("pushed origin/master.");
    }

    println!();
    println!(
        "NOTE: engine ports are fixed by the port/alias contract (:18000 etc).\n\
         Run dev (tauri dev) and prod hours alternate on engine ownership —\n\
         they must not both hold the ports at once."
    );
    Ok(())
}
