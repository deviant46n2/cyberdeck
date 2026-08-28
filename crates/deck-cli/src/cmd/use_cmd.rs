//! Apply a loadout on the live service (with optional rewire).

use anyhow::Result;

use super::with_profiles_db;

pub(crate) fn run(name: String, dry_run: bool, managed: bool) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let p = deck_core::store::get_profile(&conn, &name)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{name}'"))?;
    deck_core::store::set_active(&conn, &name)?;
    println!(
        "applying loadout '{}' (alias={}, port={}){}",
        name,
        p.alias,
        p.port,
        if dry_run { " [dry-run]" } else { "" }
    );
    deck_engines::apply(&p, dry_run)?;
    if managed && !dry_run {
        println!("MANAGED rewiring clients:");
        for r in deck_engines::rewire::rewire_clients(p.port) {
            println!("  [{}] {} — {}", r.client, r.path, r.status);
        }
    }
    Ok(())
}
