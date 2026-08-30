//! Tauri twin of the Infinite Agent Canvas (ROADMAP 8d).
//!
//! Where the CLI door (`deck workflow`) runs foreground with Ctrl-C as the
//! cancel, this door runs on background worker threads behind a registry keyed
//! by run id, surfaces progress as `wf-*` events, and offers a cooperative
//! per-run `stop`. A single-flight guard per workflow prevents double starts.
//! The domain logic (`deck_core::workflow::execute` / `deck_core::wfstore`) is
//! shared with the CLI; this file is serialization + threading only.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use tauri::Emitter;

use deck_core::store::{default_db_path, open};
use deck_core::wfstore;
use deck_core::workflow::{Workflow, WorkflowRunStatus};

#[derive(Clone, Serialize)]
pub struct WfStarted {
    pub run_id: String,
    pub workflow_id: String,
}

#[derive(Clone, Serialize)]
pub struct WfNodeEvt {
    pub run_id: String,
    pub node_id: String,
    pub ok: bool,
    pub error: String,
}

#[derive(Clone, Serialize)]
pub struct WfDoneEvt {
    pub run_id: String,
    pub workflow_id: String,
    pub status: String,
    pub tokens_used: u64,
    pub nodes_ok: u32,
    pub nodes_failed: u32,
}

struct WfJob {
    /// Cooperative stop flag the executor polls between waves.
    stop: Arc<AtomicBool>,
    /// The workflow being run — used to reject a second launch of the same wf.
    workflow_id: String,
}

/// Active background workflow runs keyed by run id.
static WF_RUNS: std::sync::LazyLock<Mutex<std::collections::HashMap<String, WfJob>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn now() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seed the built-in role set + Coding Review workflow, idempotently.
pub fn workflow_seed() -> anyhow::Result<String> {
    let db = default_db_path();
    let conn = open(&db)?;
    wfstore::ensure_wf_schema(&conn)?;
    let roles = deck_core::workflow::seed_coding_review_roles();
    let wf = deck_core::workflow::seed_coding_review();
    for r in &roles {
        wfstore::save_role(&conn, r, now())?;
    }
    wfstore::save_workflow(&conn, &wf, now())?;
    Ok(wf.id)
}

/// Import a workflow from a JSON document body. Returns the saved id.
pub fn workflow_save(body: &str) -> anyhow::Result<String> {
    let wf: Workflow = serde_json::from_str(body)?;
    let db = default_db_path();
    let conn = open(&db)?;
    wfstore::ensure_wf_schema(&conn)?;
    wfstore::save_workflow(&conn, &wf, now())?;
    Ok(wf.id)
}

pub fn workflow_list() -> anyhow::Result<Vec<Workflow>> {
    let db = default_db_path();
    let conn = open(&db)?;
    wfstore::ensure_wf_schema(&conn)?;
    wfstore::list_workflows(&conn)
}

pub fn workflow_get(id: &str) -> anyhow::Result<Option<Workflow>> {
    let db = default_db_path();
    let conn = open(&db)?;
    wfstore::ensure_wf_schema(&conn)?;
    wfstore::get_workflow(&conn, id)
}

pub fn workflow_history(workflow_id: Option<&str>) -> anyhow::Result<Vec<wfstore::WorkflowRunRow>> {
    let db = default_db_path();
    let conn = open(&db)?;
    wfstore::ensure_wf_schema(&conn)?;
    wfstore::list_workflow_runs(&conn, workflow_id)
}

/// Run `wf` once against `runner`, persisting the run + node rows. Returns the
/// executor report. Mirrors the CLI door's persistence so both doors leave the
/// same shape of rows; the caller is responsible for surfacing events.
fn execute_and_persist(
    conn: &rusqlite::Connection,
    wf: &Workflow,
    runner: &dyn deck_engines::NodeRunner,
    stop: &AtomicBool,
) -> anyhow::Result<deck_engines::ExecReport> {
    let run_id = format!("wr-{}", now());
    let run_row = wfstore::WorkflowRunRow {
        id: run_id.clone(),
        workflow_id: wf.id.clone(),
        status: WorkflowRunStatus::Running,
        created_at: now(),
        updated_at: now(),
        budget_tokens: wf.exec_settings.budget_tokens,
        tokens_used: 0,
        output: String::new(),
    };
    wfstore::insert_workflow_run(conn, &run_row)?;

    let report =
        deck_engines::execute(wf, runner, run_id.clone(), 4, stop).map_err(anyhow::Error::msg)?;

    for nr in &report.node_results {
        let nrow = wfstore::NodeRunRow {
            id: format!("{}-{}", run_id, nr.node_id),
            run_id: run_id.clone(),
            node_id: nr.node_id.clone(),
            role_id: wf.nodes
                .iter()
                .find(|n| n.id == nr.node_id)
                .map(|n| n.role_id.clone())
                .unwrap_or_default(),
            kind: wf.nodes
                .iter()
                .find(|n| n.id == nr.node_id)
                .map(|n| n.kind_label().to_string())
                .unwrap_or_default(),
            status: if nr.ok { "done" } else { "failed" }.into(),
            model_ref: wf.nodes
                .iter()
                .find(|n| n.id == nr.node_id)
                .map(|n| n.binding.model_ref.clone())
                .unwrap_or_default(),
            output: if nr.ok { nr.text.clone() } else { String::new() },
            error: nr.error.clone(),
            started_at: None,
            finished_at: Some(now()),
            attempts: 1,
            order_idx: nr.order_idx as i64,
        };
        wfstore::insert_node_run(conn, &nrow)?;
    }

    let status = report.status;
    wfstore::update_workflow_run(
        conn,
        &run_id,
        status,
        report.tokens_used,
        &format!("{:?}", status),
        now(),
    )?;

    Ok(report)
}

/// Begin a background workflow run. Returns immediately; progress flows as
/// `wf-node` / `wf-done` / `wf-error` events tagged with `run_id`. Re-launching
/// the same workflow while a run is active is rejected (single-flight).
pub fn workflow_run(
    app: &tauri::AppHandle,
    workflow_id: &str,
    runner: &str,
    dir: Option<&str>,
    model: Option<&str>,
) -> anyhow::Result<WfStarted> {
    let db = default_db_path();
    let conn = open(&db)?;
    wfstore::ensure_wf_schema(&conn)?;
    let wf = wfstore::get_workflow(&conn, workflow_id)?
        .ok_or_else(|| anyhow::anyhow!("workflow '{workflow_id}' not found"))?;

    // single-flight: reject launching the same workflow while a run is in flight
    {
        let g = WF_RUNS.lock().unwrap();
        let already = g
            .values()
            .any(|job| job.workflow_id == wf.id);
        if already {
            anyhow::bail!("workflow '{}' already running", wf.id);
        }
    }
    // Generate a run id now so the started event is stable across emit/thread.
    let run_id = format!("wr-{}", now() + 1);

    {
        let mut g = WF_RUNS.lock().unwrap();
        if g.contains_key(&run_id) {
            anyhow::bail!("workflow run '{}' already active", run_id);
        }
        g.insert(
            run_id.clone(),
            WfJob {
                stop: Arc::new(AtomicBool::new(false)),
                workflow_id: wf.id.clone(),
            },
        );
    }

    let _ = app.emit(
        "wf-start",
        WfStarted { run_id: run_id.clone(), workflow_id: wf.id.clone() },
    );

    let app2 = app.clone();
    let wf2 = wf.clone();
    let run_id2 = run_id.clone();
    let runner_s = runner.to_string();
    let dir_s = dir.map(String::from);
    let model_s = model.map(String::from);
    std::thread::spawn(move || {
        let stop = match WF_RUNS.lock().unwrap().get(&run_id2) {
            Some(j) => j.stop.clone(),
            None => return,
        };
        let db = default_db_path();
        let report = (|| -> anyhow::Result<deck_engines::ExecReport> {
            let conn = open(&db)?;
            wfstore::ensure_wf_schema(&conn)?;
            let runner_obj: &dyn deck_engines::NodeRunner = if runner_s == "agentic" {
                let dir = dir_s
                    .ok_or_else(|| anyhow::anyhow!("agentic runner needs dir"))?;
                &deck_engines::AgenticRunner { dir, model: model_s }
            } else {
                &deck_engines::StatelessRunner { max_tokens: 8192 }
            };
            execute_and_persist(&conn, &wf2, runner_obj, &stop)
        })();

        // Free the registry slot before the terminal event so an immediate
        // re-run isn't rejected as a duplicate.
        WF_RUNS.lock().unwrap().remove(&run_id2);

        match report {
            Ok(rep) => {
                for nr in &rep.node_results {
                    let _ = app2.emit(
                        "wf-node",
                        WfNodeEvt {
                            run_id: run_id2.clone(),
                            node_id: nr.node_id.clone(),
                            ok: nr.ok,
                            error: nr.error.clone(),
                        },
                    );
                }
                let nodes_ok = rep.node_results.iter().filter(|r| r.ok).count() as u32;
                let nodes_failed = rep.node_results.len() as u32 - nodes_ok;
                let _ = app2.emit(
                    "wf-done",
                    WfDoneEvt {
                        run_id: run_id2.clone(),
                        workflow_id: wf2.id.clone(),
                        status: format!("{:?}", rep.status),
                        tokens_used: rep.tokens_used,
                        nodes_ok,
                        nodes_failed,
                    },
                );
            }
            Err(e) => {
                let _ = app2.emit(
                    "wf-error",
                    WfDoneEvt {
                        run_id: run_id2.clone(),
                        workflow_id: wf2.id.clone(),
                        status: "error".into(),
                        tokens_used: 0,
                        nodes_ok: 0,
                        nodes_failed: 0,
                    },
                );
                eprintln!("workflow run {run_id2} failed: {e:#}");
            }
        }
    });

    Ok(WfStarted { run_id, workflow_id: wf.id })
}

/// Request a cooperative stop for an in-flight run (best-effort).
pub fn workflow_stop(run_id: &str) -> anyhow::Result<()> {
    let g = WF_RUNS.lock().unwrap();
    if let Some(job) = g.get(run_id) {
        job.stop.store(true, Ordering::SeqCst);
        Ok(())
    } else {
        anyhow::bail!("no active run '{}'", run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deck_core::workflow::{Message, WorkflowNode};

    struct EchoRunner;

    impl deck_engines::NodeRunner for EchoRunner {
        fn name(&self) -> &'static str { "echo" }
        fn run(&self, _node: &WorkflowNode, inputs: &[Message]) -> Result<String, String> {
            Ok(inputs.iter().map(|m| m.text.clone()).collect::<Vec<_>>().join("|"))
        }
    }

    #[test]
    fn executes_and_persists_two_node_chain() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        wfstore::ensure_wf_schema(&conn).unwrap();
        let wf = deck_core::workflow::seed_coding_review();
        let stop = AtomicBool::new(false);
        let rep = execute_and_persist(&conn, &wf, &EchoRunner, &stop).unwrap();
        assert_eq!(rep.node_results.len(), 2);
        // run row landed, terminal status folded to Done (both nodes ok)
        let runs = wfstore::list_workflow_runs(&conn, Some(&wf.id)).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, WorkflowRunStatus::Done);
        // node rows persisted
        let nodes = wfstore::list_node_runs(&conn, &runs[0].id).unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.status == "done"));
    }
}
