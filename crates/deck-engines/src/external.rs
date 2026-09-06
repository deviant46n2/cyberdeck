//! Discover and control external systemd services that run llama-server
//! but are NOT managed by cyberdeck. These are hand-rolled services the
//! user created (e.g. `llama-uncensored.service`) — cyberdeck can start
//! and stop them but does not own their unit files.

use std::process::Command;

use anyhow::{Context, Result};
use serde::Serialize;

/// An external systemd user service running llama-server, discovered
/// from `systemctl --user list-unit-files` + cgroup inspection.
#[derive(Debug, Clone, Serialize)]
pub struct ExternalService {
    /// systemd unit name (e.g. `llama-uncensored.service`)
    pub unit: String,
    /// Human-readable description from the unit file
    pub description: String,
    /// The model path parsed from ExecStart args
    pub model: Option<String>,
    /// Short display name derived from the model filename
    pub display: String,
    /// The port the server listens on (parsed from --port arg)
    pub port: Option<u16>,
    /// Whether the unit is currently active
    pub active: bool,
    /// The full ExecStart command line (for debugging)
    pub exec_start: String,
}

/// True when a unit is owned by cyberdeck (derived from the engine registry,
/// so newly added engines are excluded automatically). Managed units are
/// skipped by external discovery because the app already controls them.
fn is_cyberdeck_unit(unit: &str) -> bool {
    crate::cyberdeck_units().iter().any(|u| *u == unit)
}

/// Discover external systemd user services that run llama-server.
/// Excludes cyberdeck-managed units and non-llama-server services.
pub fn discover_external() -> Result<Vec<ExternalService>> {
    // List all user service unit files
    let out = Command::new("systemctl")
        .args(["--user", "list-unit-files", "--type=service", "--no-legend"])
        .output()
        .context("listing user unit files")?;

    if !out.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Format: "unit-name.service    enabled    enabled"
        let unit = match line.split_whitespace().next() {
            Some(u) if u.ends_with(".service") => u.to_string(),
            _ => continue,
        };

        // Skip cyberdeck-managed units
        if is_cyberdeck_unit(&unit) {
            continue;
        }

        // Check if this unit runs llama-server by inspecting ExecStart
        let exec_start = get_exec_start(&unit)?;
        if !exec_start.contains("llama-server") {
            continue;
        }

        // Check if active
        let active = is_unit_active(&unit);

        // Parse model and port from ExecStart
        let model = extract_arg(&exec_start, "-m")
            .or_else(|| extract_arg(&exec_start, "--model"));
        let port = extract_arg(&exec_start, "--port")
            .and_then(|s| s.parse::<u16>().ok());

        let display = model
            .as_ref()
            .and_then(|m| {
                std::path::Path::new(m)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| unit.replace(".service", ""));

        // Get description from systemctl show
        let description = get_unit_description(&unit)
            .unwrap_or_else(|| display.clone());

        result.push(ExternalService {
            unit,
            description,
            model,
            display,
            port,
            active,
            exec_start,
        });
    }

    // Sort by unit name for stable display
    result.sort_by(|a, b| a.unit.cmp(&b.unit));
    Ok(result)
}

/// Start an external service by its unit name.
pub fn start_service(unit: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "start", unit])
        .status()
        .context("running systemctl start")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("systemctl --user start {unit} failed (exit {status})")
    }
}

/// Stop an external service by its unit name.
pub fn stop_service(unit: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "stop", unit])
        .status()
        .context("running systemctl stop")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("systemctl --user stop {unit} failed (exit {status})")
    }
}

/// Get the ExecStart command line from a systemd unit.
/// Systemd returns a structured format:
/// `{ path=/bin/foo ; argv[]=/bin/foo --arg1 val ; ... }`
/// We extract the `argv[]` portion which contains the full command with arguments.
fn get_exec_start(unit: &str) -> Result<String> {
    let out = Command::new("systemctl")
        .args(["--user", "show", unit, "--property=ExecStart", "--value"])
        .output()
        .context("getting ExecStart")?;

    if !out.status.success() {
        anyhow::bail!("systemctl show {unit} failed");
    }

    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Extract argv[] from the systemd structured format:
    // "{ path=/bin/foo ; argv[]=/bin/foo -arg1 -arg2 ; ... }"
    if let Some(argv_start) = raw.find("argv[]=") {
        let cmd_start = argv_start + "argv[]=".len();
        // argv[] ends at the next semicolon or closing brace
        let cmd_end = raw[cmd_start..]
            .find(';')
            .map(|i| cmd_start + i)
            .or_else(|| raw[cmd_start..].find('}').map(|i| cmd_start + i))
            .unwrap_or(raw.len());
        let cmd = raw[cmd_start..cmd_end].trim().to_string();
        if !cmd.is_empty() {
            return Ok(cmd);
        }
    }

    // Fallback: try the raw value as a plain command line
    let exec = if raw.starts_with('"') && raw.ends_with('"') {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw
    };

    Ok(exec)
}

/// Get the Description property from a systemd unit.
fn get_unit_description(unit: &str) -> Option<String> {
    let out = Command::new("systemctl")
        .args(["--user", "show", unit, "--property=Description", "--value"])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let desc = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if desc.is_empty() { None } else { Some(desc) }
}

/// Check if a systemd unit is currently active.
fn is_unit_active(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Extract a CLI argument value: looks for `--port 18000` or `--port=18000`.
fn extract_arg(cmd: &str, flag: &str) -> Option<String> {
    // Try --flag=value first
    let eq_pattern = format!("{flag}=");
    if let Some(pos) = cmd.find(&eq_pattern) {
        let val_start = pos + eq_pattern.len();
        let rest = &cmd[val_start..];
        let val: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        if !val.is_empty() {
            return Some(val);
        }
    }

    // Try --flag <value>
    if let Some(pos) = cmd.find(flag) {
        let after_flag = &cmd[pos + flag.len()..];
        let after_flag = after_flag.trim_start();
        if let Some(val) = after_flag.split_whitespace().next() {
            if !val.starts_with('-') {
                return Some(val.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_port_from_exec() {
        let cmd = "/opt/llama/bin/llama-server -m /models/test.gguf --port 18999 -ngl 99";
        assert_eq!(extract_arg(cmd, "--port"), Some("18999".into()));
    }

    #[test]
    fn extract_model_short_flag() {
        let cmd = "/opt/llama/bin/llama-server -m /models/test.gguf --port 18999";
        assert_eq!(extract_arg(cmd, "-m"), Some("/models/test.gguf".into()));
    }

    #[test]
    fn extract_model_equals_form() {
        let cmd = "/opt/llama/bin/llama-server --model=/tmp/model.gguf -ngl 64";
        assert_eq!(extract_arg(cmd, "--model"), Some("/tmp/model.gguf".into()));
    }
}
