//! Agentic console: concurrent `opencode run` sessions.

use std::io::BufRead;
use std::os::unix::process::CommandExt as _;

use serde::{Serialize, Deserialize};
use tauri::Emitter;

use crate::console_reaper::AGENT_MARKER;
use crate::sessions;
use deck_core::store::SessionStatus;

/// `PR_SET_PDEATHSIG(SIGKILL)`: the agent dies the moment its parent (this app)
/// dies, by ANY exit path — clean shutdown, crash, or OOM-kill. Combined with
/// the own-process-group setup, an app death can no longer orphan an `opencode
/// run` on the GPU (observed four times; the in-app reaper only runs at startup
/// and `kill_all` only on a clean exit, so both missed crashes).
const PR_SET_PDEATHSIG: libc::c_int = 1;
const SIGKILL: libc::c_int = 9;

/// Configure an own process group + parent-death SIGKILL on `cmd`. `stderr`
/// inherits the app's, so a prctl failure is visible in the dev console even
/// though pre_exec runs in the forked (not yet exec'd) child.
fn pdeathsig(cmd: &mut std::process::Command) {
    cmd.process_group(0);
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(PR_SET_PDEATHSIG, SIGKILL as libc::c_ulong) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Race guard: if the parent died between fork and prctl, no one is
            // left to die-for — do it ourselves instead of orphaning.
            if libc::getppid() == 1 {
                libc::_exit(1);
            }
            Ok(())
        });
    }
}

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
    /// Set by `opencode_stop` so the waiter thread knows not to overwrite
    /// the DB status (stopped vs. complete/error race).
    stopped: std::sync::atomic::AtomicBool,
}

static SESSIONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Session>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Streams one session pipe (stdout or stderr) as `opencode-output` events.
/// Runs until the pipe closes. If `persist` is true, each line is also
/// written to the session_events table for handoff generation.
fn spawn_opcode_reader<R: std::io::Read + Send + 'static>(
    app: tauri::AppHandle,
    session: String,
    stream: &'static str,
    pipe: R,
    persist: bool,
) {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(pipe);
        for line in reader.lines().map_while(Result::ok) {
            let _ = app.emit(
                "opencode-output",
                OpLine {
                    session: session.clone(),
                    stream: stream.into(),
                    text: line.clone(),
                },
            );
            // Persist to DB for handoff generation.
            if persist {
                sessions::emit_session_event(&app, &session, "line", stream, &line);
            }
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
///
/// If `session_id` is provided, the session is tracked in the DB with status
/// transitions and persisted output events for handoff generation.
pub fn opencode_run(
    app: &tauri::AppHandle,
    prompt: &str,
    dir: &str,
    auto: bool,
    engine: Engine,
    model: Option<&str>,
    ctx: u32,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    let id = session_id.map(String::from).unwrap_or_else(|| {
        format!(
            "sess-{}",
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        )
    });

    // If a session_id was provided, mark it as running in the DB.
    // emit_session_status handles the DB write + frontend event.
    if session_id.is_some() {
        sessions::emit_session_status(app, &id, SessionStatus::Running, None, None);
    }
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("run").arg("--dir").arg(dir);
    cmd.env(AGENT_MARKER, &id);
    if auto {
        cmd.arg("--auto");
    }
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        cmd.arg("-m").arg(m);
    }
    cmd.arg("--ctx").arg(ctx.to_string());
    cmd.arg(prompt);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    pdeathsig(&mut cmd);

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
            stopped: std::sync::atomic::AtomicBool::new(false),
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

    spawn_opcode_reader(app.clone(), id.clone(), "stdout", stdout, session_id.is_some());
    spawn_opcode_reader(app.clone(), id.clone(), "stderr", stderr, session_id.is_some());

    let app_done = app.clone();
    let id_done = id.clone();
    let has_session = session_id.is_some();
    std::thread::spawn(move || {
        // Take the child OUT from under the lock, then drop the guard: waiting
        // on the session must not hold SESSIONS, or opencode_stop (and the
        // next spawn) would block for the whole session runtime instead of
        // being able to look up the pid and TERM→KILL it.
        let (child, stopped_flag) = {
            let mut g = SESSIONS.lock().unwrap();
            let session = g.get_mut(&id_done);
            let stopped = session
                .as_ref()
                .map(|s| s.stopped.load(std::sync::atomic::Ordering::SeqCst))
                .unwrap_or(false);
            let child = session.and_then(|s| s.child.lock().unwrap().take());
            (child, stopped)
        };
        let code = match child {
            Some(mut c) => c.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
            None => -1,
        };
        eprintln!("[deck] opencode_run: session {id_done} exited code={code}");
        SESSIONS.lock().unwrap().remove(&id_done);

        // Update session status in DB and emit event — but only if
        // opencode_stop hasn't already marked it as stopped. The stop
        // caller writes "stopped" first; the waiter must not clobber it
        // with "complete" or "error".
        if has_session && !stopped_flag {
            if code == 0 {
                sessions::emit_session_status(&app_done, &id_done, SessionStatus::Complete, Some(code), None);
            } else {
                sessions::emit_session_status(
                    &app_done,
                    &id_done,
                    SessionStatus::Error,
                    Some(code),
                    Some(&format!("exit code {code}")),
                );
            }
        }

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

/// Stop a single session by id (SIGTERM, escalating to SIGKILL since `opencode
/// run` ignores TERM and keeps streaming — observed twice). Unknown ids are
/// ignored. Multiple sessions can run; this ends only the named one.
/// If the session was tracked in the DB, it's marked as stopped.
pub fn opencode_stop(id: &str) -> anyhow::Result<()> {
    // Set the stopped flag FIRST so the waiter thread sees it before we
    // write "stopped" to the DB — prevents the waiter from overwriting
    // with "complete" or "error" after we mark stopped.
    if let Some(session) = SESSIONS.lock().unwrap().get(id) {
        session.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        term_then_kill(session.pid);
    }
    SESSIONS.lock().unwrap().remove(id);
    // Mark as stopped in DB if it exists.
    let _ = sessions::mark_session_stopped(id);
    Ok(())
}

/// True if `/proc/{pid}` still names a process whose argv starts with
/// `opencode`. Guards the SIGKILL escalation so a recycled pid that is no
/// longer our agent is never touched.
fn is_still_opencode(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
        .map(|c| c.starts_with("opencode"))
        .unwrap_or(false)
}

/// SIGTERM a session's whole process group, then escalate to SIGKILL the group
/// after a 5s grace window (on a background thread, so an interactive STOP
/// stays snappy). `opencode run` ignores SIGTERM (and its npm/python children
/// would survive a leader-only kill), so the escalation both kills hard AND
/// follows the group.
pub fn term_then_kill(pgid: u32) {
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg("--")
        .arg(format!("-{pgid}"))
        .status();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if is_still_opencode(pgid) {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg("--")
                .arg(format!("-{pgid}"))
                .status();
        }
    });
}

/// Sweep every tracked session group on app exit. Without this, a dying app
/// leaves `opencode run` children orphaned (reparented to init) and they keep
/// burning RAM indefinitely — an earlier OOM kill left two strays running for
/// hours. This runs synchronously (bounded ~3s) so the exit path actually
/// completes; a detached escalator thread would die with the process before
/// its SIGKILL. (Crash exits are covered by the pdeathsig on each run.)
pub fn kill_all() {
    let sessions = {
        let mut g = SESSIONS.lock().unwrap();
        std::mem::take(&mut *g)
    };
    if sessions.is_empty() {
        return;
    }
    eprintln!("[deck] opencode cleanup: terminating {} session(s)", sessions.len());
    for s in sessions.values() {
        let _ = std::process::Command::new("kill")
            .arg("-TERM")
            .arg("--")
            .arg(format!("-{}", s.pid))
            .status();
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    for s in sessions.values() {
        if is_still_opencode(s.pid) {
            let _ = std::process::Command::new("kill")
                .arg("-KILL")
                .arg("--")
                .arg(format!("-{}", s.pid))
                .status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_still_opencode;

    #[test]
    fn escalation_guard_only_matches_opencode_argv() {
        assert!(!is_still_opencode(1), "pid 1 is not our agent");
        assert!(!is_still_opencode(u32::MAX), "nonexistent pid");
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep");
        assert!(!is_still_opencode(child.id()), "sleep must not match");
        child.kill().ok();
        child.wait().ok();
    }
}

/// Kill a process by PID with SIGTERM, then SIGKILL after a 5s grace window
/// (on a background thread, so an interactive STOP stays snappy).
pub fn kill_process(pid: u32) {
    let _ = std::process::Command::new("kill")
        .arg("-TERM")
        .arg(format!("-{}", pid))
        .status();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .arg(format!("-{}", pid))
            .status();
    });
}

/// Tauri command: kill a process group by PID.
#[tauri::command]
pub fn tui_kill(pid: String) -> Result<(), String> {
    let pgid = pid.parse::<u32>().map_err(|e| format!("invalid pid: {e}"))?;
    kill_process(pgid);
    Ok(())
}
