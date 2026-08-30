pub use deck_core::workload::Workload;

pub fn workloads_list() -> Result<Vec<Workload>, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::ensure_seeded_workloads(&conn).map_err(|e| e.to_string())?;
    deck_core::store::list_workloads(&conn).map_err(|e| e.to_string())
}
