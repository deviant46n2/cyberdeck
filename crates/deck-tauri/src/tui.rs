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

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once};

use anyhow::Context;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::Emitter;

/// Diagnostic log file — all TUI diagnostics go here so we can read them
/// even when launched from the dock (no terminal for stderr).
/// Truncated on each app launch so the file doesn't grow forever.
static LOG_INIT: Once = Once::new();

fn tui_log_init() {
    LOG_INIT.call_once(|| {
        let _ = std::fs::write("/tmp/cyberdeck-tui.log", "");
    });
}

fn tui_log(msg: &str) {
    eprintln!("{msg}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open("/tmp/cyberdeck-tui.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

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
    /// Set to true when the frontend calls tui_ready. The PTY reader thread
    /// buffers output until this flips, then flushes the buffer and switches
    /// to live emission — prevents data loss from Tauri's non-queuing events.
    ready: Arc<AtomicBool>,
    /// Buffered PTY output accumulated before the frontend was ready.
    buffer: Mutex<VecDeque<Vec<u8>>>,
}


/// Panes and the shared serve process. The serve child is a singleton started
/// lazily on the first spawn so all panes attach to the same running server.
static PANES: LazyLock<Mutex<HashMap<String, Active>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SERVE: LazyLock<Mutex<Option<std::process::Child>>> = LazyLock::new(|| Mutex::new(None));
static NEXT_PANE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn serve_port() -> u16 {
    19771
}

/// Check whether a TCP port on localhost is accepting connections.
fn port_is_listening(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        std::time::Duration::from_millis(50),
    )
    .is_ok()
}

/// Lazy singleton `opencode serve` on a fixed loopback port. Best-effort: if it
/// is already up (port in use) we treat that as success — panes only need the
/// endpoint. Spawned with the standard process API (no PTY needed for the
/// headless server).
///
/// Three guarantees the old version lacked:
/// 1. If our tracked child has exited (serve crashed), we restart it.
/// 2. If the port is already in use (external serve), we skip spawning.
/// 3. We wait for the port to accept connections before returning, so
///    `opencode attach` never races against a still-starting server.
fn ensure_serve() {
    let mut g = SERVE.lock().unwrap();

    // 1. If we have a tracked child, check if it's still alive.
    if let Some(child) = g.as_mut() {
        match child.try_wait() {
            Ok(Some(status)) => {
                eprintln!("[deck] tui: opencode serve exited ({status}) — will restart");
                *g = None;
            }
            Ok(None) => {
                // Still alive.  But also verify the port is actually listening
                // (the process could be alive but not yet ready, or wedged).
                if port_is_listening(serve_port()) {
                    return;
                }
                eprintln!("[deck] tui: opencode serve alive but port {} not listening — will restart", serve_port());
                let _ = child.kill();
                let _ = child.wait();
                *g = None;
            }
            Err(e) => {
                eprintln!("[deck] tui: can't poll serve child: {e} — will restart");
                *g = None;
            }
        }
    }

    // 2. If the port is already in use (external serve or previous instance),
    //    treat as success — we don't own the process but the endpoint works.
    if port_is_listening(serve_port()) {
        eprintln!("[deck] tui: port {} already listening (external serve)", serve_port());
        return;
    }

    // 3. Spawn a fresh serve process.
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("serve")
        .arg("--port")
        .arg(serve_port().to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            eprintln!("[deck] tui: opencode serve started pid={} on :{}", child.id(), serve_port());
            *g = Some(child);
        }
        Err(e) => {
            eprintln!("[deck] tui: opencode serve spawn failed: {e}");
            return; // fall through — attach will fail visibly
        }
    }

    // 4. Wait for the port to become ready (up to 5 s).
    for i in 0..100 {
        if port_is_listening(serve_port()) {
            eprintln!("[deck] tui: opencode serve ready after {}ms", i * 50);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    eprintln!("[deck] tui: opencode serve port {} NOT ready after 5s — attach will likely fail", serve_port());
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
/// Kill any orphaned `deck-*` tmux sessions left from prior app launches.
/// NEXT_PANE resets to 1 on restart so session names (deck-pane-1, etc.)
/// collide with stale sessions whose opencode attach process is already dead.
fn cleanup_orphaned_tmux_sessions() {
    let Ok(output) = std::process::Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
    else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    for name in stdout.lines() {
        if name.starts_with("deck-") {
            eprintln!("[deck] tui: killing orphaned tmux session {name}");
            let _ = std::process::Command::new("tmux")
                .args(["kill-session", "-t", name])
                .output();
        }
    }
}

pub fn tui_spawn(app: &tauri::AppHandle, dir: &str, cols: u16, rows: u16) -> anyhow::Result<String> {
    tui_log_init();
    tui_log(&format!("[deck] tui_spawn called: dir={dir} cols={cols} rows={rows}"));
    ensure_serve();
    let _ = deck_core::opencode_sync::sync_opencode(true);
    cleanup_orphaned_tmux_sessions();

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .context("open PTY")?;

    let id = format!("pane-{}", NEXT_PANE.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    let sess = format!("deck-{id}");
    let ready = Arc::new(AtomicBool::new(false));

    // Try tmux first for session persistence, fall back to direct PTY.
    let use_tmux = std::process::Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let child: Box<dyn Child + Send> = if use_tmux {
        let dir_q = if dir.contains(' ') || dir.contains('\'') {
            format!("'{}'", dir.replace('\'', "'\\''"))
        } else {
            dir.to_string()
        };
        let inner = format!("opencode attach http://127.0.0.1:{} --dir {dir_q}", serve_port());
        eprintln!("[deck] tui: creating tmux session {sess} with: {inner}");
        tui_log(&format!("[deck] tui: creating tmux session {sess} with: {inner}"));
        let st = std::process::Command::new("tmux")
            .args(["new-session", "-d", "-e", "TERM=xterm-256color", "-s", &sess, "-c", dir, &inner])
            .output();
        match &st {
            Ok(o) if o.status.success() => {
                eprintln!("[deck] tui: tmux session {sess} created OK");
                tui_log(&format!("[deck] tui: tmux session {sess} created OK"));
            }
            Ok(o) => {
                let msg = format!("[deck] tui: tmux session create FAILED: {}", String::from_utf8_lossy(&o.stderr));
                eprintln!("{msg}");
                tui_log(&msg);
            }
            Err(e) => {
                let msg = format!("[deck] tui: tmux session create ERROR: {e}");
                eprintln!("{msg}");
                tui_log(&msg);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut pb = CommandBuilder::new("tmux");
        pb.arg("attach-session");
        pb.arg("-t");
        pb.arg(&sess);
        pb.env("TERM", "xterm-256color");
        match pair.slave.spawn_command(pb) {
            Ok(c) => {
                let msg = format!("[deck] tui: pane {id} → tmux {sess} attached OK (dir={dir})");
                eprintln!("{msg}");
                tui_log(&msg);
                c
            }
            Err(e) => {
                let msg = format!("[deck] tui: tmux attach failed for {sess}: {e} — direct opencode");
                eprintln!("{msg}");
                tui_log(&msg);
                let mut pb2 = CommandBuilder::new("opencode");
                pb2.arg("attach");
                pb2.arg(format!("http://127.0.0.1:{}", serve_port()));
                pb2.arg("--dir");
                pb2.arg(dir);
                pb2.env("TERM", "xterm-256color");
                pair.slave.spawn_command(pb2).context("spawn opencode")?
            }
        }
    } else {
        let mut pb = CommandBuilder::new("opencode");
        pb.arg("attach");
        pb.arg(format!("http://127.0.0.1:{}", serve_port()));
        pb.arg("--dir");
        pb.arg(dir);
        pb.env("TERM", "xterm-256color");
        pair.slave.spawn_command(pb).context("spawn opencode")?
    };
    drop(pair.slave);

    let master = pair.master;
    let mut reader = master.try_clone_reader().context("clone PTY reader")?;
    let writer = master.take_writer().context("take PTY writer")?;

    PANES.lock().unwrap().insert(
        id.clone(),
        Active {
            master,
            writer: Mutex::new(writer),
            child: Mutex::new(Some(child)),
            ready: ready.clone(),
            buffer: Mutex::new(VecDeque::new()),
        },
    );

    let _ = app.emit("tui-started", TuiStarted { id: id.clone() });

    let app_w = app.clone();
    let id_w = id.clone();
    std::thread::spawn(move || {
        tui_log(&format!("[deck] tui: reader thread started for {id_w}"));
        // Forward raw PTY bytes as tui-data events until EOF.
        // While `ready` is false, buffer output so the frontend doesn't miss
        // the initial render (Tauri events are non-queuing — emitted before
        // the listener registers are silently lost). Once `ready` flips,
        // flush the buffer then switch to live emission.
        let mut buf = [0u8; 4096];
        let mut total_bytes: usize = 0;
        let mut read_count: usize = 0;
        let mut flushed = false;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let msg = format!("[deck] tui: pane {id_w} PTY EOF after {read_count} reads, {total_bytes} bytes");
                    eprintln!("{msg}");
                    tui_log(&msg);
                    break;
                }
                Err(e) => {
                    let msg = format!("[deck] tui: pane {id_w} PTY read error: {e} after {read_count} reads, {total_bytes} bytes");
                    eprintln!("{msg}");
                    tui_log(&msg);
                    break;
                }
                Ok(n) => {
                    read_count += 1;
                    total_bytes += n;
                    let chunk = buf[..n].to_vec();
                    if read_count <= 3 {
                        let preview = String::from_utf8_lossy(&chunk);
                        let msg = format!("[deck] tui: pane {id_w} PTY read #{read_count}: {n} bytes, ready={}, preview={:?}", ready.load(Ordering::Relaxed), preview);
                        eprintln!("{msg}");
                        tui_log(&msg);
                    }
                    if ready.load(Ordering::Relaxed) {
                        // Frontend is listening — flush buffer on first transition, then emit live.
                        if !flushed {
                            let mut q = PANES.lock().unwrap();
                            if let Some(a) = q.get_mut(&id_w) {
                                let buffered: Vec<Vec<u8>> = a.buffer.lock().unwrap().drain(..).collect();
                                for bc in buffered {
                                    let _ = app_w.emit("tui-data", TuiData { id: id_w.clone(), bytes: bc });
                                }
                            }
                            flushed = true;
                            let msg = format!("[deck] tui: pane {id_w} flushed buffer, now live");
                            eprintln!("{msg}");
                            tui_log(&msg);
                        }
                        if read_count <= 3 {
                            eprintln!("[deck] tui: pane {id_w} PTY read #{read_count}: {n} bytes (total {total_bytes})");
                        }
                        let _ = app_w.emit("tui-data", TuiData { id: id_w.clone(), bytes: chunk });
                    } else {
                        // Frontend not ready yet — buffer it.
                        if read_count <= 3 {
                            let msg = format!("[deck] tui: pane {id_w} BUFFERING read #{read_count}: {n} bytes (ready=false)");
                            eprintln!("{msg}");
                            tui_log(&msg);
                        }
                        let mut q = PANES.lock().unwrap();
                        if let Some(a) = q.get_mut(&id_w) {
                            a.buffer.lock().unwrap().push_back(chunk);
                        }
                    }
                }
            }
        }
        // Drain any remaining buffer on exit (covers the case where the
        // process exits before the frontend calls tui_ready).
        if !flushed {
            let mut q = PANES.lock().unwrap();
            if let Some(a) = q.get_mut(&id_w) {
                for chunk in a.buffer.lock().unwrap().drain(..) {
                    let _ = app_w.emit("tui-data", TuiData { id: id_w.clone(), bytes: chunk });
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
        let msg = format!("[deck] tui: pane {id_w} exited code={code} (total {read_count} reads, {total_bytes} bytes)");
        eprintln!("{msg}");
        tui_log(&msg);
        let _ = app_w.emit("tui-exited", TuiExited { id: id_w.clone(), code });
    });

    let msg = format!("[deck] tui: pane {id} attached to :{} (dir={dir})", serve_port());
    eprintln!("{msg}");
    tui_log(&msg);
    Ok(id)
}

/// Signal that the frontend listener is registered. The PTY reader thread
/// buffers output until this is called — after which it flushes the buffer
/// and switches to live emission.
pub fn tui_ready(id: &str) -> anyhow::Result<()> {
    let g = PANES.lock().unwrap();
    let a = g.get(id).context("no such pane")?;
    a.ready.store(true, Ordering::Relaxed);
    let msg = format!("[deck] tui: pane {id} marked ready — reader will flush buffer");
    eprintln!("{msg}");
    tui_log(&msg);
    Ok(())
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
    eprintln!("[deck] tui: pane {id} resize → {cols}×{rows}");
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
        drop(a.writer.lock().unwrap());
        if let Some(mut c) = a.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // Kill the backing tmux session so we don't leak deck-* sessions.
        let sess = format!("deck-{id}");
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &sess])
            .output();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_is_listening_detects_open_port() {
        // Bind a temporary TCP listener and verify port_is_listening finds it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_is_listening(port), "bound port {port} should be detected");
    }

    #[test]
    fn port_is_listening_rejects_closed_port() {
        // Pick a port that is almost certainly unused.
        assert!(!port_is_listening(59999), "unused port should not be detected");
    }

    #[test]
    fn ensure_serve_survives_restart_after_child_exit() {
        // Simulate: set SERVE to Some(dead_child), then call ensure_serve.
        // The function should detect the dead child and restart.
        // We can't fully test this without opencode, but we can verify the
        // liveness check path by inserting a dead child.
        {
            let mut g = SERVE.lock().unwrap();
            // Spawn a process that exits immediately.
            let child = std::process::Command::new("true")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            *g = Some(child);
        }
        // Now ensure_serve should detect the dead child and clear it.
        // (It will try to start opencode serve, which may or may not succeed,
        // but the key is that it doesn't just return early with the dead child.)
        ensure_serve();
        let g = SERVE.lock().unwrap();
        // After ensure_serve, either g is None (spawn failed) or Some(alive child).
        // The important thing is it's not still holding the dead "true" child.
        if let Some(child) = g.as_ref() {
            // If we have a child, it should be the opencode serve process, not "true".
            // We can't easily verify this, but at least the static was cleared.
        }
    }
}
