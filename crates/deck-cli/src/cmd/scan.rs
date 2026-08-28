//! Inventory + indexing commands.

use anyhow::Result;

pub(crate) fn run() -> Result<()> {
    let roots = deck_core::scanner::default_roots();
    let mut models = deck_core::scanner::scan(&roots)?;

    // Also index ollama models.
    if let Ok(ollama) = deck_feeds::ollama_models() {
        for o in &ollama {
            let existing: std::collections::HashSet<String> = models
                .iter()
                .map(|m| {
                    std::fs::canonicalize(&m.path)
                        .ok()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| m.path.display().to_string())
                })
                .collect();
            let canonical = std::fs::canonicalize(&o.path)
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| o.path.clone());
            if !existing.contains(&canonical) {
                let meta = if let Ok(gguf_meta) = deck_core::gguf::GgufMeta::read(&o.path) {
                    gguf_meta.to_meta(&std::path::PathBuf::from(&o.path))
                } else {
                    deck_core::model::ModelMeta {
                        path: std::path::PathBuf::from(o.path.clone()),
                        format: deck_core::model::ModelFormat::Gguf,
                        name: o.name.clone(),
                        arch: None,
                        quant: None,
                        params: None,
                        n_layers: None,
                        n_embd: None,
                        ctx_train: None,
                        vocab: None,
                        weight_size: o.size,
                        footprint: o.size,
                    }
                };
                models.push(meta);
            }
        }
    }

    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    let n = deck_core::store::upsert_many(&mut conn, &models)?;
    let keep: Vec<String> = models
        .iter()
        .map(|m| m.path.display().to_string())
        .collect();
    let pruned = deck_core::store::prune(&conn, &keep)?;

    println!(
        "indexed {n} model(s), pruned {pruned} stale -> {}",
        db.display()
    );
    for m in &models {
        println!(
            "  {:<10} {:<18} {:<8} {:.2} GiB  {}",
            format!("{:?}", m.format),
            m.arch.as_deref().unwrap_or("?"),
            m.quant.as_deref().unwrap_or("?"),
            m.footprint as f64 / 1_073_741_824.0,
            m.path.display(),
        );
    }

    let dups = deck_core::store::duplicates(&conn)?;
    if !dups.is_empty() {
        println!("\nDUPLICATES (wasted space):");
        for d in &dups {
            println!(
                "  {:<14} wasted {:.2} GiB across {} copies",
                d.identity,
                d.wasted_bytes as f64 / 1_073_741_824.0,
                d.members.len()
            );
            for m in &d.members {
                println!("      {}", m.path.display());
            }
        }
    }
    Ok(())
}
