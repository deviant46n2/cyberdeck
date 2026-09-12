//! Runtime installation end to end: a manifest `url` recipe downloads a zip
//! from a local HTTP server, unpacks it, chmods the binary, and refuses a
//! wrong bin_path or a bad sha256 loudly. Skips cleanly without python3/curl.

use std::path::{Path, PathBuf};
use std::time::Duration;

use deck_engines::install::{InstallPlan, Unpack, execute};

fn have(cmd: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {cmd}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn setup(tag: &str) -> Option<(PathBuf, u16, std::process::Child)> {
    if !have("python3") || !have("curl") || !have("unzip") {
        eprintln!("skipping: need python3 + curl + unzip");
        return None;
    }
    // Unique per test: parallel tests share a process (and pid).
    let base = std::env::temp_dir().join(format!("deck-install-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let srv = base.join("srv");
    let bindir = srv.join("bin");
    std::fs::create_dir_all(&bindir).unwrap();
    std::fs::write(bindir.join("mockrt"), "#!/bin/sh\necho mockrt 1.0\n").unwrap();
    std::fs::write(srv.join("mockrt.bin"), "#!/bin/sh\necho mockrt-bin 2.0\n").unwrap();
    // Zip preserving the bin/ path (exec bit need not survive; execute chmods).
    let zippy = std::process::Command::new("python3")
        .args([
            "-c",
            "import zipfile,sys; z=zipfile.ZipFile(sys.argv[1],'w'); z.write(sys.argv[2],'bin/mockrt'); z.close()",
            &srv.join("mock.zip").display().to_string(),
            &bindir.join("mockrt").display().to_string(),
        ])
        .status();
    if !zippy.map(|s| s.success()).unwrap_or(false) {
        eprintln!("skipping: could not build fixture zip");
        return None;
    }
    let port = free_port();
    let mut child = std::process::Command::new("python3")
        .args([
            "-m", "http.server", &port.to_string(),
            "--directory", &srv.display().to_string(),
            "--bind", "127.0.0.1",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // Wait for readiness (never assume the port is up).
    let url = format!("http://127.0.0.1:{port}/mock.zip");
    let mut ready = false;
    for _ in 0..50 {
        let ok = std::process::Command::new("curl")
            .args(["-sSf", "-o", "/dev/null", &url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !ready {
        let _ = child.kill();
        eprintln!("skipping: fixture server never came up");
        return None;
    }
    Some((base, port, child))
}

fn plan(url: String, bin_path: &str, unpack: Unpack, sha256: Option<String>) -> InstallPlan {
    InstallPlan {
        filename: url.rsplit('/').next().unwrap_or("mock.zip").into(),
        url,
        unpack,
        bin_path: bin_path.into(),
        sha256,
        version: "test".into(),
    }
}

fn run_mock(bin: &Path) -> String {
    String::from_utf8_lossy(
        &std::process::Command::new(bin)
            .arg("--version")
            .output()
            .expect("installed mock must run")
            .stdout,
    )
    .trim()
    .to_string()
}

#[test]
fn zip_recipe_installs_an_executable_and_runs_it() {
    let Some((base, port, mut srv)) = setup("zip") else { return };
    let dest = base.join("dest-zip");
    let p = plan(
        format!("http://127.0.0.1:{port}/mock.zip"),
        "bin/mockrt",
        Unpack::Zip,
        None,
    );
    let bin = execute(&p, &dest, &|_| {}).expect("zip install");
    assert_eq!(run_mock(&bin), "mockrt 1.0");
    let _ = srv.kill();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn bare_file_recipe_lands_and_wrong_bin_path_fails_loudly() {
    let Some((base, port, mut srv)) = setup("bare") else { return };
    let dest = base.join("dest-bare");
    let p = plan(
        format!("http://127.0.0.1:{port}/mockrt.bin"),
        "mockrt",
        Unpack::None,
        None,
    );
    let bin = execute(&p, &dest, &|_| {}).expect("bare install");
    assert_eq!(run_mock(&bin), "mockrt-bin 2.0");

    // A wrong bin_path inside an ARCHIVE must fail loudly: the declared
    // executable has to exist after unpack (for "none" the file IS the binary,
    // so the name is just where it lands).
    let bad = plan(
        format!("http://127.0.0.1:{port}/mock.zip"),
        "bin/does-not-exist",
        Unpack::Zip,
        None,
    );
    let err = execute(&bad, &base.join("dest-bad"), &|_| {}).unwrap_err().to_string();
    assert!(
        err.contains("no executable"),
        "missing bin must fail loudly, got: {err}"
    );
    let _ = srv.kill();
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn sha256_mismatch_refuses_the_bytes() {
    let Some((base, port, mut srv)) = setup("sha") else { return };
    if !have("sha256sum") {
        eprintln!("skipping: no sha256sum tool");
        let _ = srv.kill();
        return;
    }
    let p = plan(
        format!("http://127.0.0.1:{port}/mock.zip"),
        "bin/mockrt",
        Unpack::Zip,
        Some("0".repeat(64)),
    );
    let err = execute(&p, &base.join("dest-sha"), &|_| {}).unwrap_err().to_string();
    assert!(err.contains("sha256 mismatch"), "got: {err}");
    let _ = srv.kill();
    let _ = std::fs::remove_dir_all(&base);
}
