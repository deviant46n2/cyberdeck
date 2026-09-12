//! `deck install`: fetch a runtime backend from its manifest recipe.
//! Downloads (never fabricates) via system curl, unpacks, chmods, and records
//! the binary in `engine_bin` so availability flips to installed. `manual`
//! recipes print guidance instead — cyberdeck will not fake a pip install.

use anyhow::Result;
use deck_engines::install::{Resolved, execute, resolve_recipe};

pub(crate) fn run(runtime: &str, tag: Option<&str>, dry_run: bool) -> Result<()> {
    let m = deck_core::runtime::all_manifests()
        .into_iter()
        .find(|x| x.id == runtime)
        .ok_or_else(|| anyhow::anyhow!("unknown runtime '{runtime}'"))?;
    match resolve_recipe(&m, tag)? {
        Resolved::Manual { url, instructions } => {
            println!("runtime '{runtime}' needs a manual install:");
            println!("  {url}");
            if !instructions.is_empty() {
                println!("  {instructions}");
            }
            println!("then register the binary: `deck engines bin {runtime} <path>`");
            Ok(())
        }
        Resolved::Download(plan) => {
            println!("[install] {} version {}:", runtime, plan.version);
            println!("  url: {}", plan.url);
            println!("  file: {} ({:?}) → bin {}", plan.filename, plan.unpack, plan.bin_path);
            if dry_run {
                println!("[install] --dry-run: nothing downloaded, nothing registered");
                return Ok(());
            }
            let db = deck_core::store::default_db_path();
            let conn = deck_core::store::open(&db)?;
            if let Ok(Some(cur)) = deck_core::store::installed_version(&conn, runtime)
                && cur == plan.version
            {
                println!("[install] already at {cur} — re-installing");
            }
            let dest = deck_core::runtime::runtime_install_dir(runtime);
            let bin = execute(&plan, &dest, &|s| println!("[install] {s}"))?;
            deck_core::store::record_runtime_install(
                &conn,
                runtime,
                &bin.display().to_string(),
                &plan.version,
            )?;
            println!("[install] registered {runtime} → {}", bin.display());
            Ok(())
        }
    }
}
