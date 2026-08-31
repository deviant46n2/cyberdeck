use anyhow::Result;

pub fn sync(write: bool) -> Result<()> {
    let msg = deck_core::opencode_sync::sync_opencode(write)?;
    println!("{msg}");
    Ok(())
}
