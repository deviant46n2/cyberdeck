//! End-to-end proof that a NON-builtin runtime manifest drives the real launch
//! path: discovery → argv/unit rendering → spawn on a test port → health.
//!
//! Uses a tiny python HTTP server as the "experimental runtime" (the exact
//! class of backend the redesign must make first-class). Skips cleanly when
//! python3 is unavailable so CI without it isn't a false failure.

use std::path::Path;

use deck_core::profile::Profile;

#[test]
fn custom_manifest_runs_through_the_generic_launch_path() {
    // python3 is the mock runtime's interpreter; without it, skip.
    let python = which("python3");
    let Some(python) = python else {
        eprintln!("skipping: python3 not found");
        return;
    };

    let base = std::env::temp_dir().join(format!("deck-rt-launch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let runtimes = base.join("cyberdeck/runtimes");
    std::fs::create_dir_all(&runtimes).unwrap();
    // Discovery reads XDG_DATA_HOME; this test binary owns the process env.
    unsafe { std::env::set_var("XDG_DATA_HOME", &base) };

    let port = free_port();
    let script = base.join("mock_runtime.py");
    std::fs::write(
        &script,
        r#"
import sys, http.server, socketserver
port = int(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/health':
            self.send_response(200); self.end_headers(); self.wfile.write(b'ok')
        else:
            self.send_response(404); self.end_headers()
    def log_message(self, *a): pass
with socketserver.TCPServer(("127.0.0.1", port), H) as srv:
    srv.serve_forever()
"#,
    )
    .unwrap();

    std::fs::write(
        runtimes.join("mockruntime.json"),
        format!(
            r#"{{"id":"mockruntime","display":"Mock Runtime","status":"experimental",
                "formats":["gguf"],"default_port":{},"test_port":{},
                "argv_template":["{}","{{port}}"]}}"#,
            port + 1000,
            port,
            script.display()
        ),
    )
    .unwrap();

    let p = Profile {
        runtime_id: Some("mockruntime".into()),
        model: "/tmp/whatever.gguf".into(),
        host: "127.0.0.1".into(),
        // Port the child actually binds (as `boot_on_test_port` sets it before
        // rendering argv), so the mock and the health probe agree.
        port,
        ctx_size: 2048,
        ..Default::default()
    };

    // Manifest resolved by id, unit name defaulted, argv rendered from template.
    assert_eq!(deck_engines::unit_name_for(&p), "cyberdeck-mockruntime.service");
    let args = deck_engines::args_for(&p);
    assert_eq!(args, vec![script.display().to_string(), port.to_string()]);
    let unit = deck_engines::render_unit(&p);
    assert!(unit.contains("Mock Runtime"), "unit names the runtime: {unit}");
    assert!(unit.contains(&script.display().to_string()), "ExecStart uses template");

    // The real spawn+health path a custom bringup would use.
    let child = deck_engines::spawn_and_wait(
        Path::new(&python),
        &args,
        &[],
        "127.0.0.1",
        port,
        std::time::Duration::from_secs(15),
    );
    match child {
        Ok(mut c) => {
            let _ = c.kill();
            let _ = c.wait();
        }
        Err((verdict, summary)) => {
            let _ = std::fs::remove_dir_all(&base);
            panic!("generic launch failed: {verdict}: {summary}");
        }
    }

    let _ = std::fs::remove_dir_all(&base);
}

fn which(bin: &str) -> Option<String> {
    let out = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// Reserve then release an ephemeral port.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}
