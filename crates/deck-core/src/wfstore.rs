//! Persistent storage for the Infinite Agent Canvas (ROADMAP Phase 8c).
//!
//! Roles, workflows and their runs are first-class tables. Workflow graphs
//! persist as JSON bodies (source of truth for the graph); runs persist as
//! structured rows so history/audit works without re-parsing a graph. Message
//! payloads cross edges in-memory and are snapshotted into `node_runs.output`
//! so a finished run's artifacts survive the process.
//!
//! All table creation is idempotent and ADD-only (see `store::ensure_column`),
//! consistent with the schema_version model: an older binary opening a newer
//! DB simply ignores these tables/columns.

use crate::workflow::{Role, Workflow, WorkflowRunStatus};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

pub fn ensure_wf_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS roles (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workflows (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            version INTEGER NOT NULL,
            template INTEGER NOT NULL DEFAULT 0,
            body TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workflow_runs (
            id TEXT PRIMARY KEY,
            workflow_id TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            budget_tokens INTEGER NOT NULL DEFAULT 0,
            tokens_used INTEGER NOT NULL DEFAULT 0,
            output TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS node_runs (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            role_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            model_ref TEXT NOT NULL,
            output TEXT NOT NULL,
            error TEXT NOT NULL,
            started_at INTEGER,
            finished_at INTEGER,
            attempts INTEGER NOT NULL DEFAULT 0,
            order_idx INTEGER NOT NULL DEFAULT 0
        );",
    )?;
    Ok(())
}

// ---------------------------------------------------------------- Roles

pub fn save_role(conn: &Connection, role: &Role, now: i64) -> Result<()> {
    ensure_wf_schema(conn)?;
    let body = serde_json::to_string(role)?;
    conn.execute(
        "INSERT INTO roles (id, name, body, created_at) VALUES (?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET name=?2, body=?3",
        params![role.id, role.name, body, now],
    )?;
    Ok(())
}

pub fn list_roles(conn: &Connection) -> Result<Vec<Role>> {
    ensure_wf_schema(conn)?;
    let mut stmt = conn.prepare("SELECT body FROM roles ORDER BY name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.flatten().filter_map(|b| serde_json::from_str(&b).ok()).collect())
}

pub fn get_role(conn: &Connection, id: &str) -> Result<Option<Role>> {
    ensure_wf_schema(conn)?;
    let body = conn
        .query_row("SELECT body FROM roles WHERE id=?1", [id], |r| r.get::<_, String>(0))
        .optional()?;
    Ok(body.and_then(|b| serde_json::from_str(&b).ok()))
}

pub fn delete_role(conn: &Connection, id: &str) -> Result<()> {
    ensure_wf_schema(conn)?;
    conn.execute("DELETE FROM roles WHERE id=?1", [id])?;
    Ok(())
}

// ---------------------------------------------------------------- Workflows

pub fn save_workflow(conn: &Connection, wf: &Workflow, now: i64) -> Result<()> {
    ensure_wf_schema(conn)?;
    let body = serde_json::to_string(wf)?;
    conn.execute(
        "INSERT INTO workflows (id, name, description, version, template, body, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(id) DO UPDATE SET name=?2, description=?3, version=?4, template=?5, body=?6, updated_at=?8",
        params![wf.id, wf.name, wf.description, wf.version, wf.template as i64, body, now, now],
    )?;
    Ok(())
}

pub fn list_workflows(conn: &Connection) -> Result<Vec<Workflow>> {
    ensure_wf_schema(conn)?;
    let mut stmt = conn.prepare("SELECT body FROM workflows ORDER BY updated_at DESC")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.flatten().filter_map(|b| serde_json::from_str(&b).ok()).collect())
}

pub fn get_workflow(conn: &Connection, id: &str) -> Result<Option<Workflow>> {
    ensure_wf_schema(conn)?;
    let body = conn
        .query_row("SELECT body FROM workflows WHERE id=?1", [id], |r| r.get::<_, String>(0))
        .optional()?;
    Ok(body.and_then(|b| serde_json::from_str(&b).ok()))
}

pub fn delete_workflow(conn: &Connection, id: &str) -> Result<()> {
    ensure_wf_schema(conn)?;
    conn.execute("DELETE FROM workflows WHERE id=?1", [id])?;
    Ok(())
}

// ---------------------------------------------------------------- Runs

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowRunRow {
    pub id: String,
    pub workflow_id: String,
    pub status: WorkflowRunStatus,
    pub created_at: i64,
    pub updated_at: i64,
    pub budget_tokens: u64,
    pub tokens_used: u64,
    pub output: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeRunRow {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub role_id: String,
    pub kind: String,
    pub status: String,
    pub model_ref: String,
    pub output: String,
    pub error: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub attempts: u32,
    pub order_idx: i64,
}

pub fn insert_workflow_run(conn: &Connection, row: &WorkflowRunRow) -> Result<()> {
    ensure_wf_schema(conn)?;
    conn.execute(
        "INSERT INTO workflow_runs (id, workflow_id, status, created_at, updated_at, budget_tokens, tokens_used, output)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![row.id, row.workflow_id, status_str(row.status), row.created_at, row.updated_at, row.budget_tokens as i64, row.tokens_used as i64, row.output],
    )?;
    Ok(())
}

pub fn update_workflow_run(conn: &Connection, id: &str, status: WorkflowRunStatus, tokens_used: u64, output: &str, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE workflow_runs SET status=?1, tokens_used=?2, output=?3, updated_at=?4 WHERE id=?5",
        params![status_str(status), tokens_used as i64, output, now, id],
    )?;
    Ok(())
}

pub fn list_workflow_runs(conn: &Connection, workflow_id: Option<&str>) -> Result<Vec<WorkflowRunRow>> {
    ensure_wf_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, workflow_id, status, created_at, updated_at, budget_tokens, tokens_used, output
         FROM workflow_runs ORDER BY created_at DESC",
    )?;
    if let Some(wid) = workflow_id {
        let rows = stmt.query_map([], row_from)?;
        let all: Vec<WorkflowRunRow> = rows.flatten().collect();
        Ok(all.into_iter().filter(|x| x.workflow_id == wid).collect())
    } else {
        let rows = stmt.query_map([], row_from)?;
        Ok(rows.flatten().collect())
    }
}

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<WorkflowRunRow> {
    Ok(WorkflowRunRow {
        id: r.get(0)?,
        workflow_id: r.get(1)?,
        status: status_from_str(&r.get::<_, String>(2)?),
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
        budget_tokens: r.get::<_, i64>(5)? as u64,
        tokens_used: r.get::<_, i64>(6)? as u64,
        output: r.get(7)?,
    })
}

pub fn insert_node_run(conn: &Connection, row: &NodeRunRow) -> Result<()> {
    ensure_wf_schema(conn)?;
    conn.execute(
        "INSERT INTO node_runs (id, run_id, node_id, role_id, kind, status, model_ref, output, error, started_at, finished_at, attempts, order_idx)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![row.id, row.run_id, row.node_id, row.role_id, row.kind, row.status, row.model_ref, row.output, row.error, row.started_at, row.finished_at, row.attempts as i64, row.order_idx],
    )?;
    Ok(())
}

pub fn update_node_run(conn: &Connection, id: &str, status: &str, output: &str, error: &str, finished_at: Option<i64>, attempts: u32) -> Result<()> {
    conn.execute(
        "UPDATE node_runs SET status=?1, output=?2, error=?3, finished_at=?4, attempts=?5 WHERE id=?6",
        params![status, output, error, finished_at, attempts as i64, id],
    )?;
    Ok(())
}

pub fn list_node_runs(conn: &Connection, run_id: &str) -> Result<Vec<NodeRunRow>> {
    ensure_wf_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, run_id, node_id, role_id, kind, status, model_ref, output, error, started_at, finished_at, attempts, order_idx
         FROM node_runs WHERE run_id=?1 ORDER BY order_idx, started_at",
    )?;
    let rows = stmt.query_map([run_id], |r| {
        Ok(NodeRunRow {
            id: r.get(0)?,
            run_id: r.get(1)?,
            node_id: r.get(2)?,
            role_id: r.get(3)?,
            kind: r.get(4)?,
            status: r.get(5)?,
            model_ref: r.get(6)?,
            output: r.get(7)?,
            error: r.get(8)?,
            started_at: r.get(9)?,
            finished_at: r.get(10)?,
            attempts: r.get::<_, i64>(11)? as u32,
            order_idx: r.get(12)?,
        })
    })?;
    Ok(rows.flatten().collect())
}

fn status_str(s: WorkflowRunStatus) -> &'static str {
    match s {
        WorkflowRunStatus::Queued => "queued",
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::Done => "done",
        WorkflowRunStatus::Partial => "partial",
        WorkflowRunStatus::Stopped => "stopped",
        WorkflowRunStatus::Error => "error",
    }
}

fn status_from_str(s: &str) -> WorkflowRunStatus {
    match s {
        "queued" => WorkflowRunStatus::Queued,
        "running" => WorkflowRunStatus::Running,
        "running_done" => WorkflowRunStatus::Running,
        "done" => WorkflowRunStatus::Done,
        "partial" => WorkflowRunStatus::Partial,
        "stopped" => WorkflowRunStatus::Stopped,
        "error" => WorkflowRunStatus::Error,
        _ => WorkflowRunStatus::Queued,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;
    use crate::workflow::{seed_coding_review, seed_coding_review_roles};

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        // Mirror what `store::open` guarantees on a fresh DB: `meta` exists and
        // schema_version is stamped.
        c.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        c.execute(
            "INSERT INTO meta (key,value) VALUES ('schema_version',?1)",
            [crate::store::SCHEMA_VERSION.to_string()],
        )
        .unwrap();
        c
    }

    #[test]
    fn role_roundtrip() {
        let c = conn();
        let role = seed_coding_review_roles().remove(0);
        save_role(&c, &role, 0).unwrap();
        let got = get_role(&c, &role.id).unwrap().unwrap();
        assert_eq!(got.id, role.id);
        assert_eq!(got.system_prompt, role.system_prompt);
        assert_eq!(list_roles(&c).unwrap().len(), 1);
    }

    #[test]
    fn workflow_roundtrip_and_run() {
        let wf = seed_coding_review();
        // schema_version guard must not interfere
        assert_eq!(store::schema_version(&conn()), Some(crate::store::SCHEMA_VERSION));

        let c = conn();
        save_workflow(&c, &wf, 1).unwrap();
        assert_eq!(list_workflows(&c).unwrap().len(), 1);
        let got = get_workflow(&c, &wf.id).unwrap().unwrap();
        assert_eq!(got.nodes.len(), 2);

        // run lifecycle
        let run = WorkflowRunRow {
            id: "r1".into(),
            workflow_id: wf.id.clone(),
            status: WorkflowRunStatus::Running,
            created_at: 1,
            updated_at: 1,
            budget_tokens: 0,
            tokens_used: 0,
            output: String::new(),
        };
        insert_workflow_run(&c, &run).unwrap();
        update_workflow_run(&c, "r1", WorkflowRunStatus::Done, 42, "final", 2).unwrap();
        let runs = list_workflow_runs(&c, Some(&wf.id)).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, WorkflowRunStatus::Done);
        assert_eq!(runs[0].tokens_used, 42);

        // node runs
        let nr = NodeRunRow {
            id: "nr1".into(),
            run_id: "r1".into(),
            node_id: "n1".into(),
            role_id: "primary-developer".into(),
            kind: "agentic".into(),
            status: "done".into(),
            model_ref: "qwen3.8@Q3".into(),
            output: "patch".into(),
            error: String::new(),
            started_at: Some(1),
            finished_at: Some(2),
            attempts: 1,
            order_idx: 0,
        };
        insert_node_run(&c, &nr).unwrap();
        assert_eq!(list_node_runs(&c, "r1").unwrap().len(), 1);
    }

    #[test]
    fn schema_version_stamped_on_open() {
        let c = conn();
        assert_eq!(store::schema_version(&c), Some(crate::store::SCHEMA_VERSION));
    }
}
