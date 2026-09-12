//! Loadout profile management (new / import / list).

use std::path::PathBuf;

use anyhow::Result;

use super::{parse_engine, with_profiles_db};

pub(crate) fn new(
    name: String,
    model: String,
    engine: String,
    bin: Option<PathBuf>,
    alias: String,
    port: u16,
    ctx: u32,
    ngl: u32,
    draft: Option<PathBuf>,
) -> Result<()> {
    let mut p = deck_core::profile::Profile::default();
    p.name = name.clone();
    p.engine = parse_engine(&engine)?;
    p.model = model;
    p.alias = alias;
    p.port = port;
    p.ctx_size = ctx;
    p.n_gpu_layers = ngl;
    p.draft_model = draft;
    if let Some(b) = bin {
        p.bin = b;
    } else if p.engine == deck_core::profile::Engine::FreeToken {
        p.bin = PathBuf::from("ft");
    } else if p.engine == deck_core::profile::Engine::Ollama {
        p.bin = PathBuf::from("ollama");
    }
    let (_db, mut conn) = with_profiles_db()?;
    deck_core::store::upsert_profile(&mut conn, &p)?;
    println!(
        "saved loadout '{name}' ({engine}, alias={}, port={})",
        p.alias, p.port
    );
    Ok(())
}

pub(crate) fn import(engine: String, script: PathBuf, name: String) -> Result<()> {
    let eng = parse_engine(&engine)?;
    let p = match eng {
        deck_core::profile::Engine::LlamaCpp | deck_core::profile::Engine::UncensoredLlamaCpp => {
            deck_core::importer::import_llamacpp_script(&script, &name)?
        }
        deck_core::profile::Engine::FreeToken => {
            deck_core::importer::import_freetoken_script(&script, &name)?
        }
        deck_core::profile::Engine::Ollama => {
            anyhow::bail!(
                "import supports llama.cpp / FreeToken launch scripts; Ollama models live in \
                 its own store (ollama pull)"
            )
        }
    };
    let (_db, mut conn) = with_profiles_db()?;
    deck_core::store::upsert_profile(&mut conn, &p)?;
    println!(
        "imported loadout '{}' from {} (alias={}, port={}, ctx={})",
        p.name,
        script.display(),
        p.alias,
        p.port,
        p.ctx_size
    );
    Ok(())
}

pub(crate) fn list(json: bool, model: Option<&str>) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let mut rows = deck_core::store::list_profiles_with_origin(&conn)?;
    if let Some(m) = model {
        rows.retain(|(p, _)| p.model == m);
    }
    let active = deck_core::store::active_profile(&conn)?;
    if json {
        let profiles: Vec<_> = rows.into_iter().map(|(p, _)| p).collect();
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else if rows.is_empty() {
        if model.is_some() {
            println!("no loadouts bound to '{}'", model.unwrap());
        } else {
            println!("no loadouts saved. use `deck profile import` or `deck profile new`.");
        }
    } else {
        for (p, origin) in &rows {
            let mark = if active.as_deref() == Some(&p.name) {
                "*"
            } else {
                " "
            };
            let tag = if origin == "override" { "ovr" } else { "auto" };
            println!(
                "{mark} {:<14} {:<10} {:<4} alias={:<12} port={:<6} ctx={}",
                p.name,
                format!("{:?}", p.engine),
                tag,
                p.alias,
                p.port,
                p.ctx_size
            );
        }
    }
    Ok(())
}

/// Show one loadout with its origin and, per field, AUTO value vs user override.
pub(crate) fn show(name: &str, json: bool) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let p = deck_core::store::get_profile(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{name}'"))?;
    let origin = deck_core::store::profile_origin(&conn, name)?;
    let prov = deck_core::store::profile_provenance(&conn, name)?
        .and_then(|raw| serde_json::from_str::<deck_core::library::Provenance>(&raw).ok());

    if json {
        let out = serde_json::json!({ "profile": p, "origin": origin, "provenance": prov });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("profile '{}' [origin={}]", p.name, origin);
    println!(
        "  runtime={} model={} port={} ctx={} ngl={} kv_k={} kv_v={}",
        p.runtime_key(),
        p.model,
        p.port,
        p.ctx_size,
        p.n_gpu_layers,
        p.kv_cache_type_k.as_deref().unwrap_or("-"),
        p.kv_cache_type_v.as_deref().unwrap_or("-"),
    );
    match &prov {
        None => println!("  no provenance recorded (hand-authored or pre-dates tracking)"),
        Some(pr) => {
            println!("  why: {}", pr.why);
            if pr.overridden_fields.is_empty() {
                println!("  all values AUTO (fit-derived)");
            } else {
                let base = serde_json::to_value(&pr.baseline).unwrap_or_default();
                let cur = serde_json::to_value(&p).unwrap_or_default();
                println!("  USER OVERRIDES (auto → override):");
                for f in &pr.overridden_fields {
                    let b = base.get(f).map(|v| v.to_string()).unwrap_or_default();
                    let c = cur.get(f).map(|v| v.to_string()).unwrap_or_default();
                    println!("    {f}: {b} → {c}");
                }
            }
        }
    }
    Ok(())
}

/// Clone a saved configuration under a new name (source stays untouched).
pub(crate) fn duplicate(source: &str, name: &str) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let p = deck_core::store::duplicate_profile(&conn, source, name)?;
    println!(
        "duplicated '{source}' → '{}' (runtime={} ctx={}); the source is untouched",
        p.name,
        p.runtime_key(),
        p.ctx_size
    );
    Ok(())
}
