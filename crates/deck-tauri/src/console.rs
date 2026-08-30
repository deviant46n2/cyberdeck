//! Agentic console: concurrent `opencode run` sessions.

use std::io::BufRead;

use serde::{Serialize, Deserialize};
use tauri::Emitter;

/// Emitted when a session starts, so the UI can open a tab before output flows.
#[derive(Clone, Serialize)]
pub struct OpStarted {
    pub id: String,
    pub prompt: String,
}

/// Engine pin for a chat/converse session.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub enum Engine {
    LlamaCpp,
    FreeToken,
    Ollama,
}

impl Engine {
    fn default_port(&self) -> u16 {
        match self {
            Engine::LlamaCpp => 18000,
            Engine::FreeToken => 1919,
            Engine::Ollama => 11434,
        }
    }
    fn host(&self) -> &'static str { "127.0.0.1" }
}

#[derive(Clone, Serialize)]
pub struct OpLine {
    pub session: String,
    pub stream: String,
    pub text: String,
}

#[derive(Clone, Serialize)]
pub struct OpDone {
    pub session: String,
    pub code: i32,
}

/// One concurrent opencode session. The child handle is kept so its pipes stay
/// open; stop is performed by PID (via SIGTERM) so it never contends with the
/// waiter thread that holds the lock during `wait`.
struct Session {
    pid: u32,
    engine: Engine,
    child: std::sync::Mutex<Option<std::process::Child>>,
}

static SESSIONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Session>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Env marker stamped on every spawned agent so a startup sweep can recognize
/// sessions orphaned by a crashed/crash-killed app instance (a normal exit
/// path has `kill_all`; SIGKILL has none, and a killed app leaves `opencode
/// run` children reparented to init that keep talking to the resident units).
const AGENT_MARKER: &str = "DECK_AGENT_SID";

/// Streams one session pipe (stdout or stderr) as `opencode-output` events.
/// Runs until the pipe closes.
fn spawn_opcode_reader<R: std::io::Read + Send + 'static>(
    app: tauri::AppHandle,
    session: String,
    stream: &'static str,
    pipe: R,
) {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(pipe);
        for line in reader.lines().map_while(Result::ok) {
            let _ = app.emit(
                "opencode-output",
                OpLine {
                    session: session.clone(),
                    stream: stream.into(),
                    text: line,
                },
            );
        }
    });
}

/// Spawn a new `opencode run` session in `dir`. Unlike a single-slot runner,
/// this supports many concurrent sessions: each gets a unique id, its output is
/// tagged with that id, and `opencode_stop(id)` ends just that one.
///
/// `auto` maps to opencode's `--auto` (auto-approve permissions) — required for
/// a headless coding session, but it WILL let the agent modify files without
/// prompting. The UI must surface that trade-off.
pub fn opencode_run(
    app: &tauri::AppHandle,
    prompt: &str,
    dir: &str,
    auto: bool,
    engine: Engine,
    model: Option<&str>,
) -> anyhow::Result<()> {
    let id = format!(
        "sess-{}",
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("run").arg("--dir").arg(dir);
    cmd.env(AGENT_MARKER, &id);
    if auto {
        cmd.arg("--auto");
    }
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        cmd.arg("-m").arg(m);
    }
    cmd.arg(prompt);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => {
            eprintln!("[deck] opencode_run: spawned pid={}", c.id());
            c
        }
        Err(e) => {
            eprintln!("[deck] opencode_run: SPAWN FAILED: {e}");
            return Err(anyhow::anyhow!("failed to spawn opencode: {e}"));
        }
    };
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("opencode stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("opencode stderr unavailable"))?;

    SESSIONS.lock().unwrap().insert(
        id.clone(),
        Session {
            pid,
            engine,
            child: std::sync::Mutex::new(Some(child)),
        },
    );

    eprintln!("[deck] opencode_run: emitting opencode-started id={id}");
    match app.emit(
        "opencode-started",
        OpStarted {
            id: id.clone(),
            prompt: prompt.to_string(),
        },
    ) {
        Ok(_) => eprintln!("[deck] opencode_run: emit ok id={id}"),
        Err(e) => eprintln!("[deck] opencode_run: EMIT FAILED id={id}: {e}"),
    }

    spawn_opcode_reader(app.clone(), id.clone(), "stdout", stdout);
    spawn_opcode_reader(app.clone(), id.clone(), "stderr", stderr);

    let app_done = app.clone();
    let id_done = id.clone();
    std::thread::spawn(move || {
        let code = {
            let mut g = SESSIONS.lock().unwrap();
            match g
                .get_mut(&id_done)
                .and_then(|s| s.child.lock().unwrap().take())
            {
                Some(mut c) => c.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
                None => -1,
            }
        };
        eprintln!("[deck] opencode_run: session {id_done} exited code={code}");
        SESSIONS.lock().unwrap().remove(&id_done);
        let _ = app_done.emit(
            "opencode-done",
            OpDone {
                session: id_done,
                code,
            },
        );
    });

    Ok(())
}

/// Stop a single session by id (SIGTERM to its process group). Unknown ids are
/// ignored. Multiple sessions can run; this ends only the named one.
pub fn opencode_stop(id: &str) -> anyhow::Result<()> {
    let pid = SESSIONS.lock().unwrap().get(id).map(|s| s.pid);
    if let Some(pid) = pid {
        // SIGTERM the process; the reader threads see EOF and the waiter emits done.
        let _ = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
    }
    SESSIONS.lock().unwrap().remove(id);
    Ok(())
}

/// Sweep every tracked session on app exit. Without this, a dying app leaves
/// `opencode run` children orphaned (reparented to init) and they keep burning
/// RAM indefinitely — an earlier OOM kill left two strays running for hours.
pub fn kill_all() {
    let sessions = {
        let mut g = SESSIONS.lock().unwrap();
        std::mem::take(&mut *g)
    };
    for s in sessions.values() {
        let _ = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(s.pid.to_string())
            .status();
    }
    if !sessions.is_empty() {
        eprintln!("[deck] opencode cleanup: terminated {} session(s)", sessions.len());
    }
}

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

/// Startup sweep: any `opencode run` that carries the marker but whose parent
/// chain no longer contains a live deck process was orphaned by a crashed or
/// SIGKILLed app instance. TERM them (their descendents re-parent to init and
/// are caught by the second pass), then KILL what survives. This is the abrupt-
/// death counterpart to `kill_all`; it cannot kill a session of a still-live
/// app (that chain includes `self`).
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
