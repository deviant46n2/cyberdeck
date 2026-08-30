use anyhow::Result;

pub fn list(json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_seeded_workloads(&conn)?;
    let ws = deck_core::store::list_workloads(&conn)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&ws)?);
    } else {
        for w in ws {
            println!("{:<14} {:<18} {} task(s): {}", w.id, w.label, w.tasks.len(), w.description);
            for t in &w.tasks {
                println!("  - {:<12} evaluator={}  {}", t.label, if t.evaluator.is_empty() { "lexical-placeholder" } else { &t.evaluator }, &t.prompt[..t.prompt.len().min(60)]);
            }
        }
    }
    Ok(())
}
