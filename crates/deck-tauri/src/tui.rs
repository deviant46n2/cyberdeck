//! Embedded opencode TUIs: each canvas pane runs `opencode attach` on its own
//! PTY (one shared headless `opencode serve` per app). The PTY master's raw
//! byte stream is emitted as `tui-data` events and rendered by an `xterm.js`
//! pane on the canvas; frontend keystrokes flow back through `tui_write`.
//!
//! This is flaky-by-construction work: `opencode attach` (the TUI client) and
//! `opencode serve` (the headless server) both ship with opencode, so we never
//! hand-roll the interactive session protocol — we just own the PTY plumbing
//! around the official client.
//!
//! PTY ownership: `MasterPty::take_writer` is valid exactly once, so the writer
//! is captured at spawn and held in the pane for keystroke forwarding. The
//! master handle stays for resizes. The reader is cloned out to the waiter
//! thread, which forwards bytes as events and emits `tui-exited` on EOF.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use anyhow::Context;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::Emitter;

/// Emitted for every chunk of raw PTY output from a pane, tagged by pane id.
#[derive(Clone, Serialize)]
pub struct TuiData {
    pub id: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Serialize)]
pub struct TuiStarted {
    pub id: String,
}

#[derive(Clone, Serialize)]
pub struct TuiExited {
    pub id: String,
    pub code: i32,
}

struct Active {
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Box<dyn std::io::Write + Send>>,
    child: Mutex<Option<Box<dyn Child + Send>>>,
    tmux_session: Option<String>,
}

fn tmux_available() -> bool {
    std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn tmux_session_name(id: &str) -> String {
    format!("deck-{id}")
}

/// Panes and the shared serve process. The serve child is a singleton started
/// lazily on the first spawn so all panes attach to the same running server.
static PANES: LazyLock<Mutex<HashMap<String, Active>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SERVE: LazyLock<Mutex<Option<std::process::Child>>> = LazyLock::new(|| Mutex::new(None));
static NEXT_PANE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn serve_port() -> u16 {
    19771
}

/// Lazy singleton `opencode serve` on a fixed loopback port. Best-effort: if it
/// is already up (port in use) we treat that as success — panes only need the
/// endpoint. Spawned with the standard process API (no PTY needed for the
/// headless server).
fn ensure_serve() {
    let mut g = SERVE.lock().unwrap();
    if g.is_some() {
        return;
    }
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("serve")
        .arg("--port")
        .arg(serve_port().to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            eprintln!("[deck] tui: opencode serve started on :{}", serve_port());
            *g = Some(child);
        }
        Err(e) => eprintln!("[deck] tui: opencode serve start failed (treating as already-up): {e}"),
    }
}

/// Spawn a new embedded opencode TUI pane that attaches to the shared server,
/// rooted at `dir`. Returns the pane id. The model is chosen interactively
/// inside the TUI (e.g. `/models`), just like a normal terminal opencode.
///
/// Clean-room tmux persistence (inspired by TermCanvas/Zellij, not copied):
/// if `tmux` is on PATH, each pane is backed by a detached `tmux` session
/// `deck-<pane-id>` running `opencode attach …`. The PTY we render is
/// `tmux attach-session -t <sess>`, so killing the Tauri window detaches but
/// the session lives — relaunch re-attaches. Without tmux we fall back to the
/// direct PTY child (previous behaviour).
pub fn tui_spawn(app: &tauri::AppHandle, dir: &str, cols: u16, rows: u16) -> anyhow::Result<String> {
    ensure_serve();
    // One truth: deck's vault → opencode.json. Best-effort mirror on first pane
    // so the TUI `Ask anything` model picker matches `deck workflow` / `deck bench`.
    let _ = deck_core::opencode_sync::sync_opencode(true);

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .context("open PTY")?;

    let id = format!("pane-{}", NEXT_PANE.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let sess = tmux_session_name(&id);
    let use_tmux = tmux_available();

    let child: Box<dyn Child + Send> = if use_tmux {
        // Ensure a detached tmux session exists that runs the real opencode client.
        // `has-session` fails if missing — then we create it.
        let has = std::process::Command::new("tmux")
            .args(["has-session", "-t", &sess])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !has {
            // Escape dir for shell inside tmux (tmux runs via $SHELL -c).
            let dir_q = if dir.contains(' ') || dir.contains('\'') {
                format!("'{}'", dir.replace('\'', "'\\''"))
            } else {
                dir.to_string()
            };
            let inner = format!("opencode attach http://127.0.0.1:{} --dir {dir_q}", serve_port());
            let st = std::process::Command::new("tmux")
                .args(["new-session", "-d", "-s", &sess, "-c", dir, &inner])
                .output();
            match st {
                Ok(o) if o.status.success() => eprintln!("[deck] tui: tmux session {sess} created (dir={dir})"),
                Ok(o) => {
                    eprintln!("[deck] tui: tmux new-session failed for {sess}: {}", String::from_utf8_lossy(&o.stderr));
                    // fall through to direct attach below as fallback — do not return yet
                }
                Err(e) => eprintln!("[deck] tui: tmux new-session spawn failed: {e}"),
            }
        }
        // Now attach the PTY to that session. If attach fails, fall back to direct opencode.
        let mut pb = CommandBuilder::new("tmux");
        pb.arg("attach-session");
        pb.arg("-t");
        pb.arg(&sess);
        match pair.slave.spawn_command(pb) {
            Ok(c) => {
                eprintln!("[deck] tui: pane {id} → tmux {sess} (dir={dir})");
                c
            }
            Err(e) => {
                eprintln!("[deck] tui: tmux attach failed for {sess}: {e} — falling back to direct opencode attach");
                let mut pb2 = CommandBuilder::new("opencode");
                pb2.arg("attach");
                pb2.arg(format!("http://127.0.0.1:{}", serve_port()));
                pb2.arg("--dir");
                pb2.arg(dir);
                pair.slave.spawn_command(pb2).context("spawn opencode attach (tmux fallback)")?
            }
        }
    } else {
        let mut pb = CommandBuilder::new("opencode");
        pb.arg("attach");
        pb.arg(format!("http://127.0.0.1:{}", serve_port()));
        pb.arg("--dir");
        pb.arg(dir);
        pair.slave.spawn_command(pb).context("spawn opencode attach")?
    };
    drop(pair.slave);

    let master = pair.master;
    let mut reader = master.try_clone_reader().context("clone PTY reader")?;
    let writer = master.take_writer().context("take PTY writer")?;

    let tmux_sess = if use_tmux { Some(sess.clone()) } else { None };
    PANES.lock().unwrap().insert(
        id.clone(),
        Active {
            master,
            writer: Mutex::new(writer),
            child: Mutex::new(Some(child)),
            tmux_session: tmux_sess,
        },
    );

    let _ = app.emit("tui-started", TuiStarted { id: id.clone() });

    let app_w = app.clone();
    let id_w = id.clone();
    std::thread::spawn(move || {
        // Forward raw PTY bytes as tui-data events until EOF.
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = app_w.emit("tui-data", TuiData { id: id_w.clone(), bytes: buf[..n].to_vec() });
                }
            }
        }
        let code = {
            let mut g = PANES.lock().unwrap();
            match g.get_mut(&id_w).and_then(|a| a.child.lock().unwrap().take()) {
                Some(mut c) => c.wait().map(|s| s.exit_code() as i32).unwrap_or(-1),
                None => -1,
            }
        };
        PANES.lock().unwrap().remove(&id_w);
        let _ = app_w.emit("tui-exited", TuiExited { id: id_w.clone(), code });
    });

    eprintln!("[deck] tui: pane {id} attached to :{} (dir={dir})", serve_port());
    Ok(id)
}

/// Forward keystrokes from an xterm pane into the pane's PTY master.
pub fn tui_write(id: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let g = PANES.lock().unwrap();
    let a = g.get(id).context("no such pane")?;
    use std::io::Write;
    let mut w = a.writer.lock().unwrap();
    w.write_all(bytes).context("write to PTY")?;
    w.flush().ok();
    Ok(())
}

/// Resize a pane's PTY to `cols`x`rows`.
pub fn tui_resize(id: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
    let g = PANES.lock().unwrap();
    let a = g.get(id).context("no such pane")?;
    a.master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .context("resize PTY")?;
    Ok(())
}

/// Stop a pane: kill the `opencode attach` child, drop the writer (EOF) and
/// remove the pane. If the pane was tmux-backed, also kill the detached session
/// so we don't leak `deck-*` sessions — the workflow's `tuiEdges` wiring already
/// captures intent, not the raw tmux name.
pub fn tui_stop(id: &str) -> anyhow::Result<()> {
    let mut g = PANES.lock().unwrap();
    if let Some(a) = g.remove(id) {
        let sess = a.tmux_session.clone();
        drop(a.writer.lock().unwrap());
        if let Some(mut c) = a.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(s) = sess {
            let _ = std::process::Command::new("tmux")
                .args(["kill-session", "-t", &s])
                .output();
            eprintln!("[deck] tui: tmux session {s} killed");
        }
    }
    Ok(())
}
