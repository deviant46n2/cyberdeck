//! Phase 4 deterministic recommend — no ML.
//! Aggregates per (model, engine) over a workload's tasks.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedCandidate {
    pub model: String,
    pub engine: String,
    pub runs: usize,
    pub success_rate: f64, // fraction passed evaluations
    pub mean_score: f64,
    pub p50_tok_s: Option<f64>,
    pub mean_tok_s: Option<f64>,
    pub explain: String,
}

pub fn recommend(workload_id: &str, objective: &str) -> Result<Vec<RankedCandidate>, anyhow::Error> {
    let db = crate::store::default_db_path();
    let conn = crate::store::open(&db)?;
    crate::store::ensure_seeded_workloads(&conn)?;
    let w = crate::store::get_workload(&conn, workload_id)?.ok_or_else(|| anyhow::anyhow!("unknown workload '{workload_id}'"))?;
    let task_labels: Vec<String> = w.tasks.iter().map(|t| t.label.clone()).collect();

    // fetch matrix rows for those tasks
    let placeholders = task_labels.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("SELECT model, engine, task, tok_s, wall_ms FROM matrix_runs WHERE task IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(task_labels.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<f64>>(3)?, r.get::<_, i64>(4)?))
    })?.collect::<Result<Vec<_>, _>>()?;

    if rows.is_empty() {
        anyhow::bail!("insufficient data — run `deck bench matrix --workload {workload_id}` first");
    }

    // fetch evaluations keyed by matrix_run id — need to join. For MVP we approximate:
    // success_rate from evaluations where available, else assume lexical placeholder pass.
    // We read evaluations grouped by model+engine via task match (cheap proxy).
    use std::collections::HashMap;
    let mut groups: HashMap<(String,String), Vec<(Option<f64>, bool)>> = HashMap::new();
    // load evaluations map: matrix_run -> (passed, score)
    let mut eval_map: HashMap<i64, (bool,f64)> = HashMap::new();
    if let Ok(mut s) = conn.prepare("SELECT matrix_run_id, passed, score FROM evaluations") {
        if let Ok(rm) = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, f64>(2)?))) {
            for r in rm.flatten() { eval_map.insert(r.0, (r.1, r.2)); }
        }
    }
    // matrix id lookup for each row's id — we need ids, so re-query with id
    let mut stmt2 = conn.prepare(&format!("SELECT id, model, engine, tok_s FROM matrix_runs WHERE task IN ({placeholders})"))?;
    let id_rows = stmt2.query_map(rusqlite::params_from_iter(task_labels.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<f64>>(3)?)))?.collect::<Result<Vec<_>,_>>()?;

    for (id, model, engine, tok_s) in id_rows {
        let (passed, score) = eval_map.get(&id).cloned().unwrap_or((true, 0.5));
        groups.entry((model, engine)).or_default().push((tok_s, passed));
        let _ = score;
    }

    let mut out: Vec<RankedCandidate> = Vec::new();
    for ((model, engine), vals) in groups {
        let runs = vals.len();
        let successes = vals.iter().filter(|(_, p)| *p).count();
        let success_rate = successes as f64 / runs as f64;
        let toks: Vec<f64> = vals.iter().filter_map(|(t, _)| *t).collect();
        let mean_tok_s = if toks.is_empty() { None } else { Some(toks.iter().sum::<f64>() / toks.len() as f64) };
        let mut sorted = toks.clone();
        sorted.sort_by(|a,b| a.partial_cmp(b).unwrap());
        let p50 = if sorted.is_empty() { None } else { Some(sorted[sorted.len()/2]) };
        // mean_score approximated as success_rate for now (real score avg needs eval score)
        let mean_score = success_rate;
        let explain = format!("{model} via {engine}: {success_rate:.0}% task success, {} tok/s (p50), {runs} runs", p50.map(|v| format!("{v:.1}")).unwrap_or_else(|| "—".into()));
        out.push(RankedCandidate { model, engine, runs, success_rate, mean_score, p50_tok_s: p50, mean_tok_s, explain });
    }

    match objective {
        "speed" => out.sort_by(|a,b| b.p50_tok_s.partial_cmp(&a.p50_tok_s).unwrap_or(std::cmp::Ordering::Equal)),
        "efficient" => out.sort_by(|a,b| {
            let ae = a.success_rate * a.p50_tok_s.unwrap_or(0.0);
            let be = b.success_rate * b.p50_tok_s.unwrap_or(0.0);
            be.partial_cmp(&ae).unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => out.sort_by(|a,b| b.success_rate.partial_cmp(&a.success_rate).unwrap_or(std::cmp::Ordering::Equal).then(b.p50_tok_s.partial_cmp(&a.p50_tok_s).unwrap_or(std::cmp::Ordering::Equal))),
    }
    Ok(out)
}
