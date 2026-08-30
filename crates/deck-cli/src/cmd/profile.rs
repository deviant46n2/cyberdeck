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
        deck_core::profile::Engine::LlamaCpp => {
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
    let mut profiles = deck_core::store::list_profiles(&conn)?;
    if let Some(m) = model {
        profiles.retain(|p| p.model == m);
    }
    let active = deck_core::store::active_profile(&conn)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else if profiles.is_empty() {
        if model.is_some() {
            println!("no loadouts bound to '{}'", model.unwrap());
        } else {
            println!("no loadouts saved. use `deck profile import` or `deck profile new`.");
        }
    } else {
        for p in &profiles {
            let mark = if active.as_deref() == Some(&p.name) {
                "*"
            } else {
                " "
            };
            println!(
                "{mark} {:<14} {:<10} alias={:<12} port={:<6} ctx={}",
                p.name,
                format!("{:?}", p.engine),
                p.alias,
                p.port,
                p.ctx_size
            );
        }
    }
    Ok(())
}
