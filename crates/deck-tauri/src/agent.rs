//! Phase 7a agent operator — READ/ANALYZE typed tools (no shell).
//! The agent calls these via Tauri `invoke("agent_*")` instead of bash.

use serde::Serialize;

#[derive(Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub permission: String, // READ | ANALYZE | MODIFY | EXECUTE
}

pub fn agent_tools() -> Vec<ToolDef> {
    vec![
        ToolDef { name: "hardware_profile".into(), description: "this machine's hardware_profile (gpu, vram, cpu, ram, driver)".into(), permission: "READ".into() },
        ToolDef { name: "list_models".into(), description: "installed models in ~/models with quant/arch/fit".into(), permission: "READ".into() },
        ToolDef { name: "feeds_list".into(), description: "recent releases from the catalog".into(), permission: "READ".into() },
        ToolDef { name: "feeds_rank".into(), description: "relevance-ranked releases (hardware-grounded) with optional workload hint".into(), permission: "READ".into() },
        ToolDef { name: "workloads_list".into(), description: "seeded workloads and their tasks/evaluators".into(), permission: "READ".into() },
        ToolDef { name: "bench_history".into(), description: "recent bench measurements".into(), permission: "READ".into() },
        ToolDef { name: "recommend".into(), description: "ranked candidates per workload (success_rate + tok/s) — explainable".into(), permission: "READ".into() },
        ToolDef { name: "settings_get".into(), description: "typed settings get".into(), permission: "READ".into() },
        ToolDef { name: "settings_set".into(), description: "typed settings set (audited, reversible)".into(), permission: "MODIFY".into() },
        ToolDef { name: "analyze_relevance".into(), description: "analyze whether a new release is worth testing for this hardware/workload".into(), permission: "ANALYZE".into() },
    ]
}

pub fn analyze_relevance(repo: String, workload: Option<String>) -> Result<serde_json::Value, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    let releases = deck_core::store::list_releases(&conn, 200).map_err(|e| e.to_string())?;
    let r = releases.into_iter().find(|x| x.repo == repo).ok_or_else(|| format!("release '{repo}' not in catalog — run feeds poll"))?;
    let installed = deck_core::store::list(&conn).map_err(|e| e.to_string())?.into_iter().map(|m| deck_core::relevance::Installed { name: m.name, arch: m.arch, quant: m.quant }).collect::<Vec<_>>();
    let vram = deck_core::fit::available_vram_mb(16000);
    let disk_free_mb = deck_core::fit::hw_free_disk_mb().unwrap_or(268_000);
    let best = deck_core::store::recent_bench(&conn, 20).ok().and_then(|v| v.first().map(|r| r.tps)).unwrap_or(0.0);
    let bench = deck_core::relevance::BenchBest { tok_s: if best > 0.0 { Some(best) } else { None } };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut w = deck_core::relevance::Weights::default();
    if workload.as_deref() == Some("coding") { w.family = 0.35; w.hw = 0.25; }
    let score = deck_core::relevance::score_one(&r, &installed, &bench, vram, 0.0, &w, disk_free_mb);
    // alternative: use rank to compute relative
    let _ = now;
    Ok(serde_json::json!({"release": r, "score": score, "workload": workload, "recommendation": if score.total > 0.7 && score.fits { "WORTH_TESTING" } else if score.fits { "RELEVANT" } else { "SKIP" }}))
}
