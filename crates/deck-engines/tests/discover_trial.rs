//! End-to-end configuration discovery against a mock OpenAI runtime: the trial
//! boots on its test port, takes the shared probe, and records a measured
//! (not estimated) tok/s — the empirical fact fit candidates join as TESTED.
//!
//! Skips cleanly when python3 is unavailable.

use std::time::Duration;

use deck_core::profile::Profile;
use deck_engines::discover::{TrialPlan, run_trial};

#[test]
fn discovery_measures_a_real_probe_generation() {
    let Some(python) = which("python3") else {
        eprintln!("skipping: python3 not found");
        return;
    };

    let base = std::env::temp_dir().join(format!("deck-disc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let runtimes = base.join("cyberdeck/runtimes");
    std::fs::create_dir_all(&runtimes).unwrap();
    // Discovery resolves the manifest protocol through the real registry.
    unsafe { std::env::set_var("XDG_DATA_HOME", &base) };

    let port = free_port();
    let script = base.join("mock_openai.py");
    std::fs::write(
        &script,
        r#"
import sys, http.server, socketserver, json, time
port = int(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/health':
            self.send_response(200); self.end_headers(); self.wfile.write(b'ok')
        else:
            self.send_response(404); self.end_headers()
    def do_POST(self):
        n = int(self.headers.get('Content-Length', 0) or 0)
        self.rfile.read(n)
        time.sleep(0.05)  # realistic decode latency so wall tok/s is finite
        body = {"choices": [{"message": {"content": "hello"}}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 10}}
        out = json.dumps(body).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(out)))
        self.end_headers()
        self.wfile.write(out)
    def log_message(self, *a): pass
with socketserver.TCPServer(("127.0.0.1", port), H) as srv:
    srv.serve_forever()
"#,
    )
    .unwrap();

    std::fs::write(
        runtimes.join("mockdisc.json"),
        format!(
            r#"{{"id":"mockdisc","display":"Mock Discovery","status":"experimental",
                "formats":["gguf"],"default_port":{},"test_port":{},
                "argv_template":["{}","{{port}}"]}}"#,
            port + 500,
            port,
            script.display()
        ),
    )
    .unwrap();

    // Bin resolves to the interpreter; the template's first element is the script.
    std::fs::write(base.join("py"), "").ok();
    let plan = TrialPlan {
        runtime_id: "mockdisc".into(),
        display: "Mock Discovery".into(),
        test_port: port,
        profile: Profile {
            runtime_id: Some("mockdisc".into()),
            bin: python.into(),
            model: "/x.gguf".into(),
            alias: "mock".into(),
            host: "127.0.0.1".into(),
            port,
            ctx_size: 2048,
            ..Default::default()
        },
        label: "mockdisc @ ctx 2048".into(),
    };

    let r = run_trial(&plan, 32, 1, Duration::from_secs(15), None);
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(r.boot_verdict, "RUNNING", "mock failed to boot: {}", r.boot_summary);
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].verdict, "RUNNING");
    assert_eq!(r.rows[0].task, "discovery-probe");
    let tps = r.best_tps.expect("a 10-token probe must yield a wall tok/s");
    assert!(tps > 0.0, "measured tok/s must be positive, got {tps}");
    assert_eq!(r.best_kind.as_deref(), Some("wall"), "no timings → honest wall kind");
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

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}
