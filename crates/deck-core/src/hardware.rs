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

/// Live host telemetry for the companion widget: GPU util + VRAM, RAM, CPU.
/// Sampled quickly (one nvidia-smi call + two /proc samples ~200 ms apart).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveMetrics {
    pub gpu_util: u8,
    pub vram_used_mb: u32,
    pub vram_total_mb: u32,
    pub ram_used_mb: u32,
    pub ram_total_mb: u32,
    pub cpu_pct: u8,
}

fn parse_u8(s: Option<&str>) -> u8 {
    s.and_then(|t| t.trim().parse::<u8>().ok()).unwrap_or(0)
}

fn parse_u32(s: Option<&str>) -> u32 {
    s.and_then(|t| t.trim().parse::<u32>().ok()).unwrap_or(0)
}

fn gpu_usage() -> (u8, u32, u32) {
    let csv = cmd_out(
        "nvidia-smi",
        &[
            "--query-gpu=utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ],
    );
    let mut it = csv.split(',').map(|s| s.trim());
    (
        parse_u8(it.next()),
        parse_u32(it.next()),
        parse_u32(it.next()),
    )
}

fn meminfo_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

fn ram_usage() -> (u32, u32) {
    let total_kb = meminfo_kb("MemTotal");
    let avail_kb = meminfo_kb("MemAvailable");
    (
        (total_kb.saturating_sub(avail_kb) / 1024) as u32,
        (total_kb / 1024) as u32,
    )
}

fn stat_ticks() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let cpu = text.lines().next().unwrap_or_default();
    let fields: Vec<&str> = cpu.split_whitespace().collect();
    if fields.len() < 5 {
        return (0, 0);
    }
    let total: u64 = fields[1..]
        .iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .sum();
    // fields: 0=cpu 1=user 2=nice 3=system 4=idle 5=iowait
    let idle: u64 = fields[4]
        .parse::<u64>()
        .ok()
        .map(|a| {
            a + fields
                .get(5)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .unwrap_or(0);
    (total, idle)
}

fn cpu_pct() -> u8 {
    let (t0, i0) = stat_ticks();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let (t1, i1) = stat_ticks();
    let dt = t1.saturating_sub(t0);
    let di = i1.saturating_sub(i0);
    if dt == 0 {
        return 0;
    }
    ((1.0 - di as f64 / dt as f64).clamp(0.0, 1.0) * 100.0) as u8
}

pub fn live_metrics() -> LiveMetrics {
    let (gpu_util, vram_used_mb, vram_total_mb) = gpu_usage();
    let (ram_used_mb, ram_total_mb) = ram_usage();
    LiveMetrics {
        gpu_util,
        vram_used_mb,
        vram_total_mb,
        ram_used_mb,
        ram_total_mb,
        cpu_pct: cpu_pct(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_metrics_invariants() {
        let m = live_metrics();
        assert!(m.cpu_pct <= 100, "cpu_pct={}", m.cpu_pct);
        assert!(m.gpu_util <= 100, "gpu_util={}", m.gpu_util);
        assert!(m.ram_total_mb > 0, "ram_total_mb");
        assert!(m.ram_used_mb <= m.ram_total_mb, "ram used <= total");
        // VRAM stays 0 on non-NVIDIA hosts (CI) — no assert here.
    }
}
