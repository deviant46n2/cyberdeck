//! Shared helpers for the `cmd/` command modules.

use std::path::PathBuf;

use anyhow::Result;

pub(crate) mod bench;
pub(crate) mod bringup;
pub(crate) mod fit;
pub(crate) mod list;
pub(crate) mod profile;
pub(crate) mod scan;
pub(crate) mod use_cmd;

pub(crate) fn parse_engine(s: &str) -> Result<deck_core::profile::Engine> {
    match s {
        "llamacpp" | "llama" | "llama.cpp" => Ok(deck_core::profile::Engine::LlamaCpp),
        "freetoken" | "ft" => Ok(deck_core::profile::Engine::FreeToken),
        other => anyhow::bail!("unknown engine '{other}' (llamacpp|freetoken)"),
    }
}

pub(crate) fn with_profiles_db() -> Result<(PathBuf, rusqlite::Connection)> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    Ok((db, conn))
}
