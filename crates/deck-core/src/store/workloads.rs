use anyhow::Result;
use rusqlite::Connection;

// ------------------------------------------------------------ workloads (Phase 2)
pub fn ensure_workloads_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workloads (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            description TEXT NOT NULL,
            tasks_json TEXT NOT NULL
        )",
    )?;
    Ok(())
}

pub fn upsert_workload(conn: &Connection, w: &crate::workload::Workload) -> Result<()> {
    ensure_workloads_schema(conn)?;
    let tasks_json = serde_json::to_string(&w.tasks).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO workloads (id, label, description, tasks_json) VALUES (?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET label=excluded.label, description=excluded.description, tasks_json=excluded.tasks_json",
        rusqlite::params![w.id, w.label, w.description, tasks_json],
    )?;
    Ok(())
}

pub fn list_workloads(conn: &Connection) -> Result<Vec<crate::workload::Workload>> {
    ensure_workloads_schema(conn)?;
    let mut stmt = conn.prepare("SELECT id, label, description, tasks_json FROM workloads ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        let label: String = r.get(1)?;
        let description: String = r.get(2)?;
        let tasks_json: String = r.get(3)?;
        let tasks: Vec<crate::workload::WorkloadTask> = serde_json::from_str(&tasks_json).unwrap_or_default();
        Ok(crate::workload::Workload { id, label, description, tasks })
    })?;
    Ok(rows.flatten().collect())
}

pub fn get_workload(conn: &Connection, id: &str) -> Result<Option<crate::workload::Workload>> {
    ensure_workloads_schema(conn)?;
    let mut stmt = conn.prepare("SELECT id, label, description, tasks_json FROM workloads WHERE id=?1")?;
    let mut rows = stmt.query_map([id], |r| {
        let id: String = r.get(0)?;
        let label: String = r.get(1)?;
        let description: String = r.get(2)?;
        let tasks_json: String = r.get(3)?;
        let tasks: Vec<crate::workload::WorkloadTask> = serde_json::from_str(&tasks_json).unwrap_or_default();
        Ok(crate::workload::Workload { id, label, description, tasks })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn ensure_seeded_workloads(conn: &Connection) -> Result<()> {
    ensure_workloads_schema(conn)?;
    for w in crate::workload::seeded() {
        upsert_workload(conn, &w)?;
    }
    Ok(())
}
