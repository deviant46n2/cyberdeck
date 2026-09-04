//! Manage extra scan directories.

use anyhow::Result;

pub(crate) fn list(json: bool) -> Result<()> {
    let (_, conn) = super::with_profiles_db()?;
    let dirs = deck_core::store::scan_dirs(&conn)?;
    if json {
        let arr: Vec<String> = dirs.iter().map(|p| p.display().to_string()).collect();
        println!("{}", serde_json::to_string(&arr)?);
    } else if dirs.is_empty() {
        println!("No extra scan directories configured.");
        println!("  deck dirs add <path>   to add one");
    } else {
        println!("Extra scan directories:");
        for d in &dirs {
            println!("  {}", d.display());
        }
    }
    Ok(())
}

pub(crate) fn add(path: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        anyhow::bail!("Path must be absolute, got: {path}");
    }
    if !p.is_dir() {
        anyhow::bail!("Not a directory: {path}");
    }
    let (_, conn) = super::with_profiles_db()?;
    deck_core::store::add_scan_dir(&conn, path)?;
    println!("Added: {path}");
    println!("Run `deck scan` to index models from this directory.");
    Ok(())
}

pub(crate) fn remove(path: &str) -> Result<()> {
    let (_, conn) = super::with_profiles_db()?;
    if deck_core::store::remove_scan_dir(&conn, path)? {
        println!("Removed: {path}");
    } else {
        println!("Not found: {path}");
    }
    Ok(())
}
