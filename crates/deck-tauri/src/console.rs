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
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("run").arg("--dir").arg(dir);
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

    let id = format!(
        "sess-{}",
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    SESSIONS.lock().unwrap().insert(
        id.clone(),
        Session {
            pid,
            engine,
            child: std::sync::Mutex::new(Some(child)),
        },
    );

    eprintln!("[deck] opencode_run: emitting opencode-started id={id}");
    let _ = app.emit(
        "opencode-started",
        OpStarted {
            id: id.clone(),
            prompt: prompt.to_string(),
        },
    );

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
