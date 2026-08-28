//! List indexed models.

use anyhow::Result;

pub(crate) fn run(json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let models = deck_core::store::list(&conn)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&models)?);
    } else {
        for m in &models {
            println!(
                "{:<16} {:<8} arch={:<10} ctx={:<8} {:.2} GiB  {}",
                m.name,
                m.quant.as_deref().unwrap_or("?"),
                m.arch.as_deref().unwrap_or("?"),
                m.ctx_train.unwrap_or(0),
                m.footprint as f64 / 1_073_741_824.0,
                m.path.display(),
            );
        }
    }
    Ok(())
}
