//! Orphan-agent reaper: startup sweep for `opencode run` sessions left behind
//! by a crashed or SIGKILLed app instance.
//!
//! Normal exits run [`crate::console::kill_all`]; abrupt death (an OOM kill,
//! for instance) never fires that path, so spawned agents reparent to init
//! and keep streaming against the resident units. Seen live: two stray
//! `testing` agents pinned the GPU at ~99% for ~40 minutes after a dead app.
//!
//! Every spawned agent is stamped with [`AGENT_MARKER`] in its environment.
//! On boot we sweep any tagged process whose ancestor chain no longer
//! contains a live deck process — that is exactly the orphaned set.

/// Env marker stamped on every spawned agent. Declared here (not in `console`)
/// so spawning and sweeping share one constant; `console` sets it, we read it.
pub const AGENT_MARKER: &str = "DECK_AGENT_SID";

fn ppid_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // stat is "pid (comm) state ppid ..."; comm may contain spaces/parens, so
    // parse from the last ')'.
    let after = stat[stat.rfind(')')? + 1..].trim_start();
    after.split_whitespace().nth(1)?.parse().ok()
}

/// True if the process carries our agent marker in its environment.
fn wants_marker(pid: u32) -> bool {
    let env = match std::fs::read(format!("/proc/{pid}/environ")) {
        Ok(e) => e,
        Err(_) => return false, // gone (or not ours to read) — nothing to kill
    };
    let needle = format!("{AGENT_MARKER}=");
    env.split(|&b| b == 0)
        .any(|kv| kv.starts_with(needle.as_bytes()))
}

/// Walk the ancestor chain of `pid`; true if it contains `target` (a live app
/// instance owns the agent) before hitting pid 1 (no owner — orphaned).
fn chained_to(pid: u32, target: u32) -> bool {
    let mut cur = Some(pid);
    while let Some(p) = cur {
        if p == target {
            return true;
        }
        if p <= 1 {
            return false;
        }
        cur = ppid_of(p);
    }
    false
}

/// One-shot sweep for agents orphaned by a crashed/killed app instance.
/// TERM the orphans (their descendents re-parent to init and are caught by
/// the second pass), then KILL what survives. A still-live app's sessions own
/// the chain (they contain `self`) and are skipped.
pub fn reap_orphans() {
    let self_pid = std::process::id();
    let sweep = || {
        let mut orphans = Vec::new();
        if let Ok(rd) = std::fs::read_dir("/proc") {
            for e in rd.flatten() {
                let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
                    continue;
                };
                if pid == self_pid || pid <= 1 {
                    continue;
                }
                if wants_marker(pid) && !chained_to(pid, self_pid) {
                    orphans.push(pid);
                }
            }
        }
        orphans
    };
    for round in 0..2 {
        let orphans = sweep();
        if orphans.is_empty() {
            return;
        }
        eprintln!("[deck] orphan sweep: {} stale agent(s) (round {})", orphans.len(), round + 1);
        for pid in &orphans {
            let _ = std::process::Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status();
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    for pid in sweep() {
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_and_chain_detection() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .env(AGENT_MARKER, "t1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        // the environ is only stamped after the child execs; poll briefly
        // instead of racing fork->exec
        for _ in 0..50 {
            if wants_marker(pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(wants_marker(pid), "tagged child must be recognized");
        let self_pid = std::process::id();
        assert!(chained_to(pid, self_pid), "live child is owned by us");
        assert!(!chained_to(pid, u32::MAX), "stray ancestor must not match");
        assert_eq!(ppid_of(pid), Some(self_pid), "direct child of the test proc");
        child.kill().ok();
        child.wait().ok();
        assert!(ppid_of(pid).is_none(), "dead pid must be unreadable");

        let mut plain = std::process::Command::new("sleep")
            .arg("30")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        assert!(!wants_marker(plain.id()), "untagged child must not match");
        plain.kill().ok();
        plain.wait().ok();
    }
}