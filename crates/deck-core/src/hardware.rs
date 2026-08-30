//! Phase 3 hardware profile — persistent, content-hashed, FK-linked.
//! Captures enough to contextualize benchmarks; version change → new row (history survives).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HardwareProfile {
    pub id: i64,
    pub gpu: String,
    pub vram_mb: u64,
    pub cpu: String,
    pub ram_mb: u64,
    pub os: String,
    pub driver: String,
    pub cuda: String,
    pub cyberdeck_ver: String,
    pub engines_json: String, // {"llamacpp":"b1234",...}
    pub captured_at: i64,
    pub content_hash: String,
}

fn cmd_out(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default().trim().to_string()
}

fn gpu_info() -> (String, u64, String) {
    let name = cmd_out("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader,nounits"]);
    let vram = cmd_out("nvidia-smi", &["--query-gpu=memory.total", "--format=csv,noheader,nounits"]).trim().parse::<u64>().unwrap_or(0);
    let driver = cmd_out("nvidia-smi", &["--query-gpu=driver_version", "--format=csv,noheader,nounits"]);
    (if name.is_empty() { "unknown".into() } else { name.lines().next().unwrap_or("unknown").to_string() }, vram, driver)
}

fn cpu_info() -> String {
    std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())).unwrap_or_else(|| cmd_out("uname", &["-p"]))
}

fn ram_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().find(|l| l.starts_with("MemTotal")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).map(|kb| kb / 1024)).unwrap_or(0)
}

fn os_info() -> String {
    std::fs::read_to_string("/etc/os-release").ok().and_then(|s| s.lines().find(|l| l.starts_with("PRETTY_NAME")).map(|l| l.to_string())).unwrap_or_else(|| cmd_out("uname", &["-a"]))
}

fn cuda_ver() -> String { cmd_out("nvidia-smi", &[]).lines().find(|l| l.contains("CUDA Version")).unwrap_or("").to_string() }

fn engines_json() -> String {
    let mut m = serde_json::Map::new();
    for (id, bin) in [("llamacpp", "llama-server"), ("ollama", "ollama")] {
        let v = cmd_out(bin, &["--version"]);
        if !v.is_empty() { m.insert(id.to_string(), serde_json::Value::String(v.lines().next().unwrap_or("").to_string())); }
    }
    serde_json::Value::Object(m).to_string()
}

pub fn capture() -> HardwareProfile {
    let (gpu, vram, driver) = gpu_info();
    let profile = HardwareProfile {
        id: 0,
        gpu, vram_mb: vram,
        cpu: cpu_info(),
        ram_mb: ram_mb(),
        os: os_info(),
        driver,
        cuda: cuda_ver(),
        cyberdeck_ver: env!("CARGO_PKG_VERSION").to_string(),
        engines_json: engines_json(),
        captured_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0),
        content_hash: String::new(),
    };
    let hash_src = format!("{}|{}|{}|{}|{}|{}|{}|{}", profile.gpu, profile.vram_mb, profile.cpu, profile.ram_mb, profile.os, profile.driver, profile.cuda, profile.engines_json);
    let hash = format!("{:x}", hash_src.bytes().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64)));
    HardwareProfile { content_hash: hash, ..profile }
}
