//! Mirror cyberdeck's vault into opencode's model config.
//!
//! One truth: deck's `profiles` + `ollama list` are the inventory.
//! `opencode.json` is a view. This sync writes `model`/`small_model`/
//! `agent.*.model` to the alias that deck considers resident, so the mini
//! `tmux` TUIs (`tui.rs` `opencode attach`) see the same models
//! `deck workflow` does. Dry-run by default; `--write` commits.

use std::path::PathBuf;

use anyhow::{Context, Result};

fn opencode_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("OPENCODE_CONFIG") {
        return PathBuf::from(p);
    }
    dirs_next()
}

fn dirs_next() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/opencode/opencode.json")
}

/// Build the opencode model id deck considers canonical.
/// Prefers the active profile's `alias` with its engine prefix
/// (`llamacpp/<alias>` or `ollama/<alias>`), else first profile,
/// else `ollama/qwen3.8:27b` as a stable fallback that we know
/// `ollama list` has on this machine.
fn deck_canonical_model() -> String {
    let db_path = crate::store::default_db_path();
    if let Ok(conn) = crate::store::open(&db_path) {
        let _ = crate::store::ensure_profile_schema(&conn);
        if let Ok(Some(active)) = crate::store::active_profile(&conn) {
            if let Ok(Some(p)) = crate::store::get_profile(&conn, &active) {
                return format!("{}/{}", p.engine.store_id(), p.alias);
            }
        }
        if let Ok(profiles) = crate::store::list_profiles(&conn) {
            if let Some(p) = profiles.first() {
                return format!("{}/{}", p.engine.store_id(), p.alias);
            }
        }
    }
    // fallback — matches `ollama list` on this host; stateless demo uses it
    "ollama/qwen3.8:27b".into()
}

pub fn sync_opencode(write: bool) -> Result<String> {
    let cfg_path = opencode_config_path();
    let canonical = deck_canonical_model();

    // Read or create a minimal config if missing.
    let mut cfg: serde_json::Value = if cfg_path.exists() {
        let s = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("read {}", cfg_path.display()))?;
        serde_json::from_str(&s).with_context(|| format!("parse {}", cfg_path.display()))?
    } else {
        serde_json::json!({})
    };

    let before = cfg.clone();

    // Top-level `model` / `small_model` are what `opencode attach` uses for
    // the Generalist. Keep them in sync.
    let obj = cfg.as_object_mut().ok_or_else(|| anyhow::anyhow!("opencode.json is not an object"))?;
    obj.insert("model".into(), serde_json::Value::String(canonical.clone()));
    obj.insert("small_model".into(), serde_json::Value::String(canonical.clone()));

    // `agent.generalist.model` is the per-agent override that the TUI shows
    // in the footer ("Generalis·Qwen3.8-27B …").
    if let Some(agent) = obj.get_mut("agent").and_then(|v| v.as_object_mut()) {
        if let Some(generalist) = agent.get_mut("generalist").and_then(|v| v.as_object_mut()) {
            generalist.insert("model".into(), serde_json::Value::String(canonical.clone()));
        }
    }

    let diff = if before == cfg {
        format!("opencode.json already mirrors deck → {canonical} (no change)")
    } else {
        let mut lines = vec![format!("deck canonical → {canonical}")];
        let b = before.get("model").and_then(|v| v.as_str()).unwrap_or("(missing)");
        let a = cfg.get("model").and_then(|v| v.as_str()).unwrap_or("");
        if b != a {
            lines.push(format!("model: {b} → {a}"));
        }
        let bb = before
            .pointer("/agent/generalist/model")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing)");
        let aa = cfg
            .pointer("/agent/generalist/model")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if bb != aa {
            lines.push(format!("agent.generalist.model: {bb} → {aa}"));
        }
        lines.join("\n")
    };

    if write && before != cfg {
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent).context("create opencode config dir")?;
        }
        let out = serde_json::to_string_pretty(&cfg).context("serialize opencode.json")?;
        std::fs::write(&cfg_path, out + "\n").with_context(|| format!("write {}", cfg_path.display()))?;
        Ok(format!("{diff}\nwrote {}", cfg_path.display()))
    } else if write {
        Ok(diff)
    } else {
        let suffix = format!("\n(dry-run: add --write to write {})", cfg_path.display());
        Ok(format!("{diff}{suffix}"))
    }
}
