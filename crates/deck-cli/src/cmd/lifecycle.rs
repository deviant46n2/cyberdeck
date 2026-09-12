//! Model lifecycle CLI doors: forget vs remove-files, storage, runtimes,
//! make-it-work (dry-run fit ranking). Thin wrappers over deck-core so the
//! CLI and the Tauri app share one truth.

use std::path::PathBuf;

use anyhow::{Context, Result};

pub(crate) fn forget(path: &str) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let rows = deck_core::store::delete_model(&conn, path, false)?;
    println!("forgot {path} ({rows} record(s) removed; files left on disk)");
    Ok(())
}

pub(crate) fn remove_files(paths: &[String]) -> Result<()> {
    let owned: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let plan = deck_core::storage::removal_plan(&owned);
    if plan.files.is_empty() && plan.missing.is_empty() {
        println!("nothing to remove");
        return Ok(());
    }
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let mut freed = 0u64;
    for f in &plan.files {
        deck_core::store::delete_model(&conn, &f.path, false)?;
        let p = PathBuf::from(&f.path);
        if p.is_dir() {
            std::fs::remove_dir_all(&p)
                .with_context(|| format!("removing dir {}", f.path))?;
        } else {
            std::fs::remove_file(&p)
                .with_context(|| format!("removing file {}", f.path))?;
        }
        freed += f.bytes;
        println!(
            "removed {} ({:.2} GiB)",
            f.path,
            f.bytes as f64 / 1_073_741_824.0
        );
    }
    for m in &plan.missing {
        println!("missing (not on disk, record kept): {m}");
    }
    println!("freed {:.2} GiB", freed as f64 / 1_073_741_824.0);
    Ok(())
}

pub(crate) fn storage(json: bool) -> Result<()> {
    let mut roots = deck_core::scanner::default_roots();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    roots.extend(deck_core::store::scan_dirs(&conn)?);
    let found = deck_core::scanner::scan(&roots)?;
    let registered = deck_core::store::list(&conn)?;
    drop(conn);
    let canon = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .map(|c| c.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    };
    let reg: Vec<(String, String, u64, String)> = registered
        .iter()
        .map(|m| {
            let raw = m.path.display().to_string();
            (raw.clone(), canon(&m.path), m.footprint, m.name.clone())
        })
        .collect();
    let fnd: Vec<(String, String, u64, String)> = found
        .iter()
        .map(|m| {
            let raw = m.path.display().to_string();
            (raw.clone(), canon(&m.path), m.footprint, m.name.clone())
        })
        .collect();
    let r = deck_core::storage::reconcile_registered_vs_found(&reg, &fnd);
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }
    println!(
        "ok: {} ({:.2} GiB)  total on disk: {:.2} GiB",
        r.ok_count,
        r.ok_bytes as f64 / 1_073_741_824.0,
        r.total_bytes as f64 / 1_073_741_824.0
    );
    for m in &r.missing {
        println!("MISSING (db claims, file gone): {} [{:.2} GiB]", m.path, m.claimed_bytes as f64 / 1_073_741_824.0);
    }
    for o in &r.orphaned {
        println!("ORPHANED (on disk, unregistered): {} [{:.2} GiB]", o.path, o.bytes as f64 / 1_073_741_824.0);
    }
    Ok(())
}

pub(crate) fn runtimes(json: bool, probe: bool) -> Result<()> {
    use deck_engines::availability as av;
    let manifests = deck_core::runtime::all_manifests();
    let builtin: std::collections::HashSet<String> = deck_core::profile::Engine::all()
        .into_iter()
        .map(|e| e.store_id().to_string())
        .collect();
    let dirs = av::system_path_dirs();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).ok();

    let mut arr = Vec::new();
    let mut lines = Vec::new();
    for m in &manifests {
        let ov = conn
            .as_ref()
            .and_then(|c| deck_core::store::get_engine_bin(c, &m.id).ok().flatten());
        let a = av::availability_for(m, ov.as_deref(), &dirs);
        // `--probe` spawns `<bin> --version`; explicit because a manifest that
        // points at a daemon would otherwise start serving.
        let version = if probe {
            a.bin.as_deref().and_then(av::probe_version)
        } else {
            None
        };
        let bin = a.bin.map(|p| p.display().to_string());
        arr.push(serde_json::json!({
            "id": m.id, "display": m.display, "status": m.status.tag(),
            "default_port": m.default_port, "test_port": m.test_port,
            "formats": m.formats, "capabilities": m.capabilities,
            "custom": !builtin.contains(&m.id),
            "installed": a.installed,
            "bin": bin.as_deref(), "version": version.as_deref(),
        }));
        let mut line = format!(
            "{} [{}] :{} / test :{}  {} {}",
            m.id,
            m.status.tag(),
            m.default_port,
            m.test_port,
            if a.installed { "installed" } else { "missing  " },
            bin.as_deref().unwrap_or("-"),
        );
        if let Some(v) = &version {
            line.push_str(&format!("  ({v})"));
        }
        if !m.capabilities.is_empty() {
            line.push_str(&format!("  {}", m.capabilities.join(",")));
        }
        lines.push(line);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }
    for l in &lines {
        println!("{l}");
    }
    Ok(())
}

pub(crate) fn runtimes_updates() -> Result<()> {
    use deck_engines::install::{is_newer, latest_matching_tag};
    let manifests = deck_core::runtime::all_manifests();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let mut checked = 0;
    for m in &manifests {
        let (repo, pattern) = match &m.install {
            Some(deck_core::runtime::InstallRecipe::GithubRelease {
                repo,
                asset_pattern,
                ..
            }) => (repo, asset_pattern),
            _ => continue,
        };
        checked += 1;
        let installed = deck_core::store::installed_version(&conn, &m.id)
            .ok()
            .flatten();
        let installed = match installed {
            Some(v) => v,
            None => {
                println!(
                    "{}: no recorded install — `deck install {}` to baseline",
                    m.id, m.id
                );
                continue;
            }
        };
        match latest_matching_tag(repo, pattern) {
            Ok(latest) if is_newer(&latest, &installed) => println!(
                "{}: update available {} → {} (`deck install {}`)",
                m.id, installed, latest, m.id
            ),
            Ok(_) => println!("{}: current ({})", m.id, installed),
            Err(e) => println!("{}: check failed: {e}", m.id),
        }
    }
    if checked == 0 {
        println!("no runtimes with github-release recipes");
    }
    Ok(())
}

pub(crate) fn make_it_work(model: PathBuf, json: bool) -> Result<()> {
    let meta = if model.is_dir() {
        deck_core::safetensors::open_dir(&model)?
    } else {
        deck_core::gguf::GgufMeta::read(&model)?.to_meta(&model)
    };
    let vram = deck_core::fit::hw_vram().unwrap_or(12 * 1024);
    let mut cands = deck_core::fitplan::candidates_for(&meta, &deck_core::runtime::all_manifests(), vram);
    // Empirical beats estimated: freshest successful measurement per runtime —
    // plus install status, so a recommendation never points at a missing binary.
    let path = model.display().to_string();
    if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path()) {
        for c in &mut cands {
            c.tested = deck_core::store::latest_tested(&conn, &path, &c.runtime_id)
                .ok()
                .flatten();
        }
        deck_core::fitplan::attach_availability(
            &conn,
            &deck_core::runtime::system_path_dirs(),
            &mut cands,
        );
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&cands)?);
        return Ok(());
    }
    if cands.is_empty() {
        println!("no viable runtime/configuration on this machine (vram={vram} MiB)");
        return Ok(());
    }
    for c in &cands {
        let evidence = match &c.tested {
            Some(t) => format!(" · TESTED {:.1} tok/s ({}) @ctx{}", t.tps, t.kind, t.ctx),
            None => " · estimated only".to_string(),
        };
        let present = if c.installed {
            format!(" · {}", c.bin.as_deref().unwrap_or("installed"))
        } else {
            " · NOT INSTALLED".to_string()
        };
        println!(
            "{} [{}]: ctx {}{}{} — {}",
            c.display, c.status, c.max_ctx, evidence, present, c.why
        );
    }
    println!("\nrecommended: {} (ctx {})", cands[0].display, cands[0].max_ctx);
    Ok(())
}
