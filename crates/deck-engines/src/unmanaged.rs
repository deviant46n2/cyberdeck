//! Discover and control ad-hoc llama-server processes that were started
//! directly (not via systemd). These eat VRAM but are invisible to the
//! PORT MAP — this module gives cyberdeck visibility into them so the
//! user can stop them without hunting through `ps` or `htop`.

use std::process::Command;

use anyhow::{Context, Result};
use serde::Serialize;

/// An ad-hoc llama-server process not managed by cyberdeck's own systemd
/// units. May still live inside a user-managed systemd unit (e.g.
/// `llama-uncensored.service`) — in that case `systemd_unit` is populated so
/// `stop_pid` can use `systemctl stop` instead of raw SIGTERM (which would
/// trigger `Restart=on-failure` and bring the process right back).
#[derive(Debug, Clone, Serialize)]
pub struct UnmanagedProcess {
    pub pid: u32,
    /// The port the server is listening on (parsed from CLI args).
    pub port: Option<u16>,
    /// The model path (parsed from -m / --model arg).
    pub model: Option<String>,
    /// Short display name derived from the model filename.
    pub display: String,
    /// Full command line for debugging.
    pub cmd: String,
    /// If the process lives inside a systemd user unit, its name
    /// (e.g. `llama-uncensored.service`) — extracted from the cgroup.
    pub systemd_unit: Option<String>,
}

/// Discover all running llama-server processes that are NOT managed by
/// systemd. Checks each PID's cgroup to filter out systemd-managed units.
pub fn discover_unmanaged() -> Result<Vec<UnmanagedProcess>> {
    // Find all llama-server processes (full command line)
    let out = Command::new("pgrep")
        .args(["-a", "llama-server"])
        .output()
        .context("running pgrep")?;

    if !out.status.success() {
        // pgrep exits 1 when no processes found — that's fine
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // pgrep -a output: "<pid> <full command line>"
        let (pid_str, cmd) = match line.split_once(' ') {
            Some((p, c)) => (p, c),
            None => continue,
        };
        let pid: u32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Check if managed by systemd via cgroup
        if is_systemd_managed(pid) {
            continue;
        }

        let port = extract_arg(cmd, "--port")
            .and_then(|s| s.parse::<u16>().ok());
        let model = extract_arg(cmd, "-m")
            .or_else(|| extract_arg(cmd, "--model"));

        let display = model
            .as_ref()
            .and_then(|m| {
                std::path::Path::new(m)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| format!("pid:{pid}"));

        // Extract the systemd unit name from the cgroup so we can use
        // `systemctl stop` instead of raw SIGTERM.
        let systemd_unit = extract_systemd_unit(pid);

        result.push(UnmanagedProcess {
            pid,
            port,
            model,
            display,
            cmd: cmd.to_string(),
            systemd_unit,
        });
    }

    Ok(result)
}

/// Start (or restart) an unmanaged process by its systemd unit. Only works
/// for processes that live inside a systemd user unit (e.g.
/// `llama-uncensored.service`) — truly ad-hoc processes have no stored
/// command line so we cannot reconstruct the launch.
pub fn start_unit(pid: u32) -> Result<()> {
    let unit = extract_systemd_unit(pid)
        .context("process has no systemd unit — cannot start an ad-hoc process")?;

    let status = Command::new("systemctl")
        .args(["--user", "start", &unit])
        .status()
        .context("running systemctl start")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("systemctl --user start {unit} failed (exit {status})")
    }
}

/// Stop an unmanaged process. If it lives inside a systemd user unit, uses
/// `systemctl --user stop` for a clean shutdown (avoids `Restart=on-failure`
/// bringing it right back). Falls back to SIGTERM for truly ad-hoc processes.
pub fn stop_pid(pid: u32) -> Result<()> {
    // Validate the PID exists and looks like a llama-server before stopping
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .context("checking process")?;

    if !out.status.success() {
        anyhow::bail!("process {pid} not found");
    }

    let args = String::from_utf8_lossy(&out.stdout);
    if !args.contains("llama-server") {
        anyhow::bail!("process {pid} is not a llama-server (got: {args})");
    }

    // Try systemctl stop first if the process is in a systemd unit — this
    // avoids triggering Restart=on-failure.
    if let Some(unit) = extract_systemd_unit(pid) {
        let status = Command::new("systemctl")
            .args(["--user", "stop", &unit])
            .status()
            .context("running systemctl stop")?;
        if status.success() {
            return Ok(());
        }
        // If systemctl fails (unit not found, permission denied), fall
        // through to raw SIGTERM as a best-effort.
    }

    // Raw SIGTERM for truly ad-hoc processes (not in any systemd unit)
    Command::new("kill")
        .arg(pid.to_string())
        .output()
        .context("sending SIGTERM")?;

    Ok(())
}

/// Check if a PID is managed by cyberdeck's own systemd units by inspecting
/// its cgroup. Ownership is derived from the engine registry (see
/// `crate::cyberdeck_units`), so every managed engine is filtered — a PID
/// whose cgroup names one of those units is never shown as unmanaged.
fn is_systemd_managed(pid: u32) -> bool {
    let cgroup_path = format!("/proc/{pid}/cgroup");
    let Ok(content) = std::fs::read_to_string(&cgroup_path) else {
        return false;
    };

    crate::cyberdeck_units()
        .iter()
        .any(|unit| content.contains(unit))
}

/// Extract the systemd unit name from a PID's cgroup. Returns the unit name
/// (e.g. `llama-uncensored.service`) if the process lives inside a systemd
/// user slice, `None` otherwise.
fn extract_systemd_unit(pid: u32) -> Option<String> {
    let cgroup_path = format!("/proc/{pid}/cgroup");
    let content = std::fs::read_to_string(&cgroup_path).ok()?;

    // systemd v2+ cgroup format: `0::/user.slice/user-NNN.slice/user@NNN.service/app.slice/<unit>`
    // Also handles cgroupv1: `net_cls:/user.slice/user-NNN.slice/user@NNN.service/app.slice/<unit>`
    for line in content.lines() {
        // Find the last path component — it's the unit name
        if let Some(pos) = line.rfind('/') {
            let unit = line[pos + 1..].trim();
            if unit.ends_with(".service") && !unit.is_empty() {
                return Some(unit.to_string());
            }
        }
    }
    None
}

/// Extract a CLI argument value: looks for `--port 18000` or `--port=18000`.
fn extract_arg(cmd: &str, flag: &str) -> Option<String> {
    // Try --flag=value first
    let eq_pattern = format!("{flag}=");
    if let Some(pos) = cmd.find(&eq_pattern) {
        let val_start = pos + eq_pattern.len();
        let rest = &cmd[val_start..];
        // Take until next space or end
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
            // Don't return if it looks like another flag
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
    fn extract_port_long_form() {
        assert_eq!(extract_arg("llama-server --port 18000", "--port"), Some("18000".into()));
    }

    #[test]
    fn extract_port_equals() {
        assert_eq!(extract_arg("llama-server --port=18999 -t 8", "--port"), Some("18999".into()));
    }

    #[test]
    fn extract_model_short() {
        assert_eq!(extract_arg("llama-server -m /path/to/model.gguf", "-m"), Some("/path/to/model.gguf".into()));
    }

    #[test]
    fn extract_missing() {
        assert_eq!(extract_arg("llama-server -t 8", "--port"), None);
    }
}
