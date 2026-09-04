//! `deck workflow` — the CLI door for the Infinite Agent Canvas (ROADMAP 8c).
//!
//! V1 headless scope: save/list/run/history. `run` is foreground by design so a
//! Ctrl-C is the cancel affordance; a background run registry (with non-blocking
//! `stop`) arrives with the Tauri door, where runs live on worker threads.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use anyhow::{Context, Result};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn save(seed: bool, file: Option<&Path>) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    if seed {
        let roles = deck_core::workflow::seed_coding_review_roles();
        let wf = deck_core::workflow::seed_coding_review();
        for r in &roles {
            deck_core::wfstore::save_role(&conn, r, now())?;
        }
        deck_core::wfstore::save_workflow(&conn, &wf, now())?;
        println!("seeded {} role(s) and workflow '{}'", roles.len(), wf.id);
        return Ok(());
    }
    let file = file
        .ok_or_else(|| anyhow::anyhow!("provide --file <workflow.json> or --seed"))?;
    let body = std::fs::read_to_string(file)
        .with_context(|| format!("read {}", file.display()))?;
    let wf: deck_core::workflow::Workflow =
        serde_json::from_str(&body).context("parse workflow JSON")?;
    deck_core::wfstore::save_workflow(&conn, &wf, now())?;
    println!("saved workflow '{}' ({})", wf.id, wf.name);
    Ok(())
}

pub fn list(json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let wfs = deck_core::wfstore::list_workflows(&conn)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&wfs)?);
        return Ok(());
    }
    if wfs.is_empty() {
        println!("no workflows; run `deck workflow save --seed`");
        return Ok(());
    }
    for w in &wfs {
        let cycle = if w.has_cycle() { " [CYCLE]" } else { "" };
        println!(
            "{:<16} {:<20} {}{} node(s), {} edge(s)",
            w.id,
            w.name,
            cycle,
            w.nodes.len(),
            w.edges.len()
        );
        for n in &w.nodes {
            println!(
                "    - {:<10} {}  {}", 
                n.id,
                n.kind_label(),
                n.binding.model_ref
            );
        }
        for e in &w.edges {
            let cond = match &e.condition {
                Some(c) if c.trim().is_empty() => String::new(),
                Some(c) => format!(" ?{c}"),
                None => String::new(),
            };
            let lp = if e.loop_edge { " [loop]" } else { "" };
            println!("      {} -> {}{}{}", e.from, e.to, cond, lp);
        }
    }
    Ok(())
}

pub fn run(id: &str, runner: &str, dir: Option<&Path>, model: Option<&str>, task: Option<&str>) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let mut wf = deck_core::wfstore::get_workflow(&conn, id)?
        .ok_or_else(|| anyhow::anyhow!("workflow '{id}' not found"))?;
    if let Some(t) = task.filter(|s| !s.trim().is_empty()) {
        wf.inputs.insert("task".into(), t.into());
    }
    // Structural validation (Phase 8f): raw cycles refused, loop rules + edge
    // predicates checked before any node runs.
    wf.validate().map_err(anyhow::Error::msg)?;

    // Persist a run row so `history` captures this attempt.
    let run_id = format!("wr-{}", now());
    let run_row = deck_core::wfstore::WorkflowRunRow {
        id: run_id.clone(),
        workflow_id: wf.id.clone(),
        status: deck_core::workflow::WorkflowRunStatus::Running,
        created_at: now(),
        updated_at: now(),
        budget_tokens: wf.exec_settings.budget_tokens,
        tokens_used: 0,
        output: String::new(),
    };
    deck_core::wfstore::insert_workflow_run(&conn, &run_row)?;

    let stop = AtomicBool::new(false);

    let report = match runner {
        "agentic" => {
            let dir = dir.ok_or_else(|| anyhow::anyhow!("agentic runner needs --dir <workspace>"))?;
            let agentic = deck_engines::AgenticRunner { dir: dir.display().to_string(), model: model.map(String::from) };
            anyhow::Ok(deck_engines::execute(&wf, &agentic, run_id.clone(), 4, &stop).map_err(anyhow::Error::msg)?)
        }
        "echo" => {
            let echo = deck_engines::EchoRunner;
            anyhow::Ok(deck_engines::execute(&wf, &echo, run_id.clone(), 4, &stop).map_err(anyhow::Error::msg)?)
        }
        _ => {
            let stateless = deck_engines::StatelessRunner { max_tokens: 8192 };
            anyhow::Ok(deck_engines::execute(&wf, &stateless, run_id.clone(), 4, &stop).map_err(anyhow::Error::msg)?)
        }
    }?;

    // Persist node + workflow-run results.
    for nr in &report.node_results {
        let nrow = deck_core::wfstore::NodeRunRow {
            id: format!("{}-{}-{}", run_id, nr.node_id, nr.order_idx),
            run_id: run_id.clone(),
            node_id: nr.node_id.clone(),
            role_id: wf.nodes.iter().find(|n| n.id == nr.node_id).map(|n| n.role_id.clone()).unwrap_or_default(),
            kind: wf.nodes.iter().find(|n| n.id == nr.node_id).map(|n| n.kind_label().to_string()).unwrap_or_default(),
            status: if nr.ok { "done" } else { "failed" }.into(),
            model_ref: wf.nodes.iter().find(|n| n.id == nr.node_id).map(|n| n.binding.model_ref.clone()).unwrap_or_default(),
            output: if nr.ok { nr.text.clone() } else { String::new() },
            error: nr.error.clone(),
            started_at: None,
            finished_at: Some(now()),
            attempts: 1,
            order_idx: nr.order_idx as i64,
        };
        deck_core::wfstore::insert_node_run(&conn, &nrow)?;
    }
    // Phase 8e: record a per-role bench row for each engine-backed node so
    // matrix_runs accumulates "which model best at which role" for the canvas.
    let hw_id = deck_core::store::capture_hardware_profile(&conn).ok();
    for nr in &report.node_results {
        if let Some(row) = deck_engines::node_to_matrix_row(&wf, nr, now(), hw_id, None) {
            deck_core::store::insert_matrix_run(&conn, &row)?;
        }
    }
    // Persist the run's terminal status. `report.status` already folds
    // per-node failures into Partial/Stopped — do not conflate "all nodes ran"
    // with "all nodes succeeded".
    let status = report.status;
    deck_core::wfstore::update_workflow_run(
        &conn,
        &run_id,
        status,
        report.tokens_used,
        &format!("{:?}", status),
        now(),
    )?;

    // Surface results.
    for nr in &report.node_results {
        let mark = if nr.skipped {
            "skip"
        } else if nr.ok {
            "ok"
        } else {
            "ERR"
        };
        if nr.skipped {
            println!("[{}] {:<10} skipped (gated out)", mark, nr.node_id);
            continue;
        }
        println!(
            "[{}] {:<10} {:.0}ms  {}",
            mark, nr.node_id, nr.wall_ms, nr.error
        );
        if nr.ok && !nr.text.is_empty() {
            let snippet: String = nr.text.chars().take(160).collect();
            println!("      {}", snippet.replace('\n', " "));
        }
    }
    println!(
        "workflow '{}' run {}: {:?} in {:.1}s, {} tokens ({})",
        wf.id,
        run_id,
        status,
        report.total_wall_ms as f64 / 1000.0,
        report.tokens_used,
        if report.iterations > 1 {
            format!("{} loop iterations", report.iterations)
        } else {
            "no loop".into()
        }
    );
    Ok(())
}

pub fn history(wf: Option<&str>, json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let runs = deck_core::wfstore::list_workflow_runs(&conn, wf)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }
    for r in &runs {
        println!(
            "{:<14} {:<16} {:<8} tok={} {}",
            r.id,
            r.workflow_id,
            format!("{:?}", r.status),
            r.tokens_used,
            if r.output.is_empty() { "" } else { &r.output }
        );
    }
    Ok(())
}

/// Per-role bench (8e): which model best at which node for this workflow, from
/// matrix_runs accumulated across runs.
pub fn bench(id: &str, json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let wf = deck_core::wfstore::get_workflow(&conn, id)?
        .ok_or_else(|| anyhow::anyhow!("workflow '{id}' not found"))?;
    let mut roles: Vec<&str> = Vec::new();
    for n in &wf.nodes {
        if !n.role_id.is_empty() && !roles.contains(&n.role_id.as_str()) {
            roles.push(n.role_id.as_str());
        }
    }
    let rows = deck_core::store::per_role_bench(&conn, &roles)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no per-role bench yet for '{}' — run it (stateless) a few times first",
            wf.id
        );
        return Ok(());
    }
    for r in &rows {
        println!(
            "{:<12} {:<10} tok/s best {:.1} avg {:.1} last {:.1}  ({} runs, {}ms)",
            r.role_id, r.model, r.best_tps, r.avg_tps, r.last_tps, r.runs, r.last_wall_ms
        );
    }
    Ok(())
}