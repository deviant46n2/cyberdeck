use anyhow::Result;
use rusqlite::Connection;

// ---------------------------------------------------------------- profiles

/// A flavor = a named loadout bound to a vault model. `model_id` is the hinge:
/// one `models` row hosts N flavors (different ctx/engine variants of the same
/// file), and switching between them is `deck use <name>`. It is backfilled by
/// matching `body.model` against `models.path` on schema touch, so existing
/// databases link up without a destructive migration.
pub fn ensure_profile_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS profiles (
            name TEXT PRIMARY KEY,
            engine TEXT NOT NULL,
            body TEXT NOT NULL,
            model_id INTEGER REFERENCES models(id)
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    // Pre-upgrade DBs predate the column → add it, then link existing rows.
    let has_column = conn
        .prepare("PRAGMA table_info(profiles)")
        .and_then(|mut stmt| {
            Ok::<_, rusqlite::Error>(
                stmt.query_map([], |r| r.get::<_, String>(1))?
                    .flatten()
                    .any(|c| c == "model_id"),
            )
        })?;
    if !has_column {
        conn.execute(
            "ALTER TABLE profiles ADD COLUMN model_id INTEGER REFERENCES models(id)",
            [],
        )?;
    }
    crate::store::ensure_column(conn, "profiles", "origin", "TEXT NOT NULL DEFAULT 'auto'")?;
    crate::store::ensure_column(conn, "profiles", "provenance", "TEXT")?;
    super::runs::ensure_runs_schema(conn)?;
    backfill_model_ids(conn)?;
    Ok(())
}

/// Configuration origin: 'auto' (fit-engine recommended) vs 'override'
/// (user touched at least one field). Defaults to 'auto' for old rows.
pub fn profile_origin(conn: &Connection, name: &str) -> Result<String> {
    Ok(conn
        .prepare("SELECT origin FROM profiles WHERE name = ?1")?
        .query_row([name], |r| r.get::<_, String>(0))
        .unwrap_or_else(|_| "auto".to_string()))
}

/// Raw provenance JSON for a profile (the auto baseline + why), if recorded.
pub fn profile_provenance(conn: &Connection, name: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT provenance FROM profiles WHERE name = ?1")?;
    let mut rows = stmt.query_map([name], |r| r.get::<_, Option<String>>(0))?;
    Ok(rows.next().transpose()?.flatten())
}

fn set_profile_provenance(conn: &Connection, name: &str, json: &str, origin: &str) -> Result<()> {
    conn.execute(
        "UPDATE profiles SET provenance = ?2, origin = ?3 WHERE name = ?1",
        rusqlite::params![name, json, origin],
    )?;
    Ok(())
}

/// Persist a fit-derived loadout as the AUTO baseline: origin `auto` and the
/// full provenance (runtime/artifact/vram/why + the baseline config itself).
pub fn save_profile_derived(
    conn: &Connection,
    profile: &crate::profile::Profile,
    provenance: &crate::library::Provenance,
) -> Result<()> {
    upsert_profile(conn, profile)?;
    let origin = if provenance.overridden_fields.is_empty() {
        "auto"
    } else {
        "override"
    };
    set_profile_provenance(conn, &profile.name, &serde_json::to_string(provenance)?, origin)
}

/// Persist a user-edited loadout: diff it against the stored auto baseline and
/// record which fields the user actually changed. A hand-authored profile with
/// no baseline keeps origin `auto` (there is nothing to deviate from).
pub fn save_profile_edited(
    conn: &Connection,
    profile: &crate::profile::Profile,
) -> Result<Vec<String>> {
    upsert_profile(conn, profile)?;
    let Some(raw) = profile_provenance(conn, &profile.name)? else {
        return Ok(Vec::new());
    };
    let mut prov: crate::library::Provenance = serde_json::from_str(&raw).unwrap_or_default();
    prov.overridden_fields = crate::library::diff_fields(&prov.baseline, profile);
    let origin = if prov.overridden_fields.is_empty() {
        "auto"
    } else {
        "override"
    };
    set_profile_provenance(conn, &profile.name, &serde_json::to_string(&prov)?, origin)?;
    Ok(prov.overridden_fields)
}

/// Clone a saved configuration under a new name, provenance included, so the
/// source stays known-good while the copy is edited. Fails if the target name
/// already exists or the source does not.
pub fn duplicate_profile(
    conn: &Connection,
    source: &str,
    new_name: &str,
) -> Result<crate::profile::Profile> {
    if get_profile(conn, new_name)?.is_some() {
        anyhow::bail!("a loadout named '{new_name}' already exists");
    }
    let mut p = get_profile(conn, source)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{source}'"))?;
    p.name = new_name.to_string();
    upsert_profile(conn, &p)?;
    // Copy provenance (baseline + overrides) so origin semantics carry over.
    if let Some(raw) = profile_provenance(conn, source)? {
        let origin = profile_origin(conn, source)?;
        set_profile_provenance(conn, new_name, &raw, &origin)?;
    }
    Ok(p)
}

/// Full profiles paired with their origin tag, for list views that show the
/// AUTO/OVERRIDE badge without a second query.
pub fn list_profiles_with_origin(conn: &Connection) -> Result<Vec<(crate::profile::Profile, String)>> {
    Ok(list_profiles(conn)?
        .into_iter()
        .map(|p| {
            let origin = profile_origin(conn, &p.name).unwrap_or_else(|_| "auto".into());
            (p, origin)
        })
        .collect())
}



/// Resolve the vault row for a profile's model string (real local file/dir
/// paths only — remote HF ids and empty drafts stay unlinked). Returns None
/// when there is nothing to link.
pub fn resolve_model_id(conn: &Connection, model: &str) -> Result<Option<i64>> {
    let p = std::path::Path::new(model);
    if !p.is_absolute() || !p.exists() {
        return Ok(None);
    }
    Ok(conn
        .prepare("SELECT id FROM models WHERE path = ?1")?
        .query_row([model], |r| r.get::<_, i64>(0))
        .ok())
}

/// Convergence rule: materialize a minimal vault row for a local model file so
/// an applied loadout is guaranteed to have a vault entry (the Ollama-blob case
/// stops living off-book). Returns the model_id, or None for non-local paths.
pub fn ensure_model_indexed(conn: &Connection, model: &str) -> Result<Option<i64>> {
    if let Some(id) = resolve_model_id(conn, model)? {
        return Ok(Some(id));
    }
    let p = std::path::Path::new(model);
    if !p.is_absolute() || !p.exists() {
        return Ok(None);
    }
    let format = if p.is_dir() {
        "safetensors-dir"
    } else {
        "gguf"
    };
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO models (path, format, name, scanned_at)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(path) DO NOTHING",
        rusqlite::params![model, format, name, stamp],
    )?;
    resolve_model_id(conn, model)
}

/// Re-link a profile to its vault row after saving/applying (cheap idempotent —
/// a profile saved while the file was remote picks up the row once it exists).
pub fn ensure_profile_model(conn: &Connection, profile: &crate::profile::Profile) -> Result<()> {
    if profile.model.trim().is_empty() {
        return Ok(());
    }
    if let Some(id) = ensure_model_indexed(conn, &profile.model)? {
        conn.execute(
            "UPDATE profiles SET model_id = ?2 WHERE name = ?1",
            rusqlite::params![profile.name, id],
        )?;
    }
    Ok(())
}

/// One-time + idempotent link of existing profiles to their vault rows.
fn backfill_model_ids(conn: &Connection) -> Result<()> {
    let profiles = list_profiles(conn)?;
    for p in profiles {
        let _ = ensure_profile_model(conn, &p);
    }
    Ok(())
}

pub fn upsert_profile(conn: &Connection, profile: &crate::profile::Profile) -> Result<()> {
    let body = serde_json::to_string(profile)?;
    // The runtime key (custom manifest id when bound) is the joinable identity;
    // the sentinel `engine` enum is a launch detail, not the record's name.
    let engine = profile.runtime_key();
    let model_id = ensure_model_indexed(conn, &profile.model)?;
    conn.execute(
        "INSERT INTO profiles (name, engine, body, model_id) VALUES (?1,?2,?3,?4)
         ON CONFLICT(name) DO UPDATE SET engine=?2, body=?3, model_id=?4",
        rusqlite::params![profile.name, engine, body, model_id],
    )?;
    Ok(())
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<crate::profile::Profile>> {
    let mut stmt = conn.prepare("SELECT body FROM profiles ORDER BY name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for body in rows.flatten() {
        if let Ok(p) = serde_json::from_str::<crate::profile::Profile>(&body) {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn get_profile(conn: &Connection, name: &str) -> Result<Option<crate::profile::Profile>> {
    let mut stmt = conn.prepare("SELECT body FROM profiles WHERE name = ?1")?;
    let mut rows = stmt.query_map([name], |r| r.get::<_, String>(0))?;
    if let Some(body) = rows.next().transpose()? {
        return Ok(Some(serde_json::from_str::<crate::profile::Profile>(
            &body,
        )?));
    }
    Ok(None)
}

/// The saved profile currently bound to (or merely matching) a model path —
/// resident binding first, then any profile for that model. Shared by the CLI
/// and the app so both doors resolve "what is this model running as" alike.
pub fn current_profile_for_model(
    conn: &Connection,
    model: &str,
) -> Result<Option<crate::profile::Profile>> {
    let profiles = list_profiles(conn)?;
    let residents = crate::store::list_residents(conn)?;
    for r in &residents {
        if let Some(p) = profiles.iter().find(|p| p.name == r.profile && p.model == model) {
            return Ok(Some(p.clone()));
        }
    }
    Ok(profiles.into_iter().find(|p| p.model == model))
}

pub fn delete_profile(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM profiles WHERE name = ?1", [name])?;
    Ok(())
}

pub fn set_active(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('active_profile', ?1)
         ON CONFLICT(key) DO UPDATE SET value=?1",
        [name],
    )?;
    Ok(())
}

pub fn active_profile(conn: &Connection) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = 'active_profile'")?;
    let mut rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.next().transpose()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_gguf(tag: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cyberdeck-flavor-{tag}-{}.gguf", std::process::id()));
        std::fs::write(&path, b"dummy gguf").expect("temp file");
        let model = path.display().to_string();
        (path, model)
    }

    #[test]
    fn flavor_linkage_and_convergence() {
        let conn = Connection::open_in_memory().expect("mem");
        crate::store::ensure_models_table(&conn).unwrap();
        ensure_profile_schema(&conn).unwrap();
        let (path, model) = temp_gguf("a");

        let a = crate::profile::Profile {
            name: "qwen-14k".into(),
            model: model.clone(),
            ..Default::default()
        };
        upsert_profile(&conn, &a).unwrap();

        // saving a local model materializes its vault row and links the flavor
        let mid: Option<i64> = conn
            .query_row(
                "SELECT model_id FROM profiles WHERE name = 'qwen-14k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mid = mid.expect("linked");
        let vault_rows: i64 = conn
            .query_row("SELECT count(*) FROM models WHERE path = ?1", [&model], |r| r.get(0))
            .unwrap();
        assert_eq!(vault_rows, 1);

        // second flavor of the SAME model shares the vault row
        let b = crate::profile::Profile {
            name: "qwen-32k".into(),
            ..a.clone()
        };
        upsert_profile(&conn, &b).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM profiles WHERE model_id = ?1",
                [mid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);

        // remote HF ids and empty drafts stay unlinked
        let remote = crate::profile::Profile {
            name: "remote-1".into(),
            model: "Qwen/Qwen3.8-27B-Instruct".into(),
            ..a.clone()
        };
        upsert_profile(&conn, &remote).unwrap();
        let rmid: Option<i64> = conn
            .query_row("SELECT model_id FROM profiles WHERE name = 'remote-1'", [], |r| r.get(0))
            .unwrap();
        assert!(rmid.is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn ensure_schema_migrates_and_backfills_model_ids() {
        let conn = Connection::open_in_memory().expect("mem");
        crate::store::ensure_models_table(&conn).unwrap();
        // simulate a pre-model_id DB (profiles without the FK column)
        conn.execute_batch(
            "CREATE TABLE profiles (name TEXT PRIMARY KEY, engine TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        let (path, model) = temp_gguf("b");
        conn.execute(
            "INSERT INTO models (path, format) VALUES (?1, 'gguf')",
            [&model],
        )
        .unwrap();
        let p = crate::profile::Profile {
            name: "old-flavor".into(),
            model: model.clone(),
            ..Default::default()
        };
        let body = serde_json::to_string(&p).unwrap();
        conn.execute(
            "INSERT INTO profiles (name, engine, body) VALUES ('old-flavor', 'llamacpp', ?1)",
            [&body],
        )
        .unwrap();

        ensure_profile_schema(&conn).unwrap();
        let mid: Option<i64> = conn
            .query_row("SELECT model_id FROM profiles WHERE name = 'old-flavor'", [], |r| r.get(0))
            .unwrap();
        assert!(mid.is_some(), "backfill should link the pre-existing profile");

        std::fs::remove_file(&path).ok();
    }

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().expect("mem");
        crate::store::ensure_models_table(&conn).unwrap();
        ensure_profile_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn derived_baseline_then_edit_marks_overrides() {
        let conn = mem();
        let base = crate::profile::Profile {
            name: "q".into(),
            ctx_size: 49152,
            ..Default::default()
        };
        let prov = crate::library::Provenance {
            runtime_id: "llamacpp".into(),
            artifact_path: "/m.gguf".into(),
            vram_mb: 16303,
            why: "fits".into(),
            baseline: base.clone(),
            overridden_fields: vec![],
        };
        save_profile_derived(&conn, &base, &prov).unwrap();
        assert_eq!(profile_origin(&conn, "q").unwrap(), "auto");

        // Change only ctx: exactly that field becomes an override.
        let mut edited = base.clone();
        edited.ctx_size = 65536;
        let fields = save_profile_edited(&conn, &edited).unwrap();
        assert_eq!(fields, vec!["ctx_size".to_string()]);
        assert_eq!(profile_origin(&conn, "q").unwrap(), "override");

        let stored: crate::library::Provenance =
            serde_json::from_str(&profile_provenance(&conn, "q").unwrap().unwrap()).unwrap();
        assert_eq!(stored.overridden_fields, vec!["ctx_size".to_string()]);
        assert_eq!(stored.baseline.ctx_size, 49152, "auto baseline preserved");
    }

    #[test]
    fn duplicate_copies_config_and_provenance_source_untouched() {
        let conn = mem();
        let p = crate::profile::Profile {
            name: "known-good".into(),
            ctx_size: 49152,
            ..Default::default()
        };
        let prov = crate::library::Provenance {
            runtime_id: "llamacpp".into(),
            baseline: p.clone(),
            ..Default::default()
        };
        save_profile_derived(&conn, &p, &prov).unwrap();

        let dup = duplicate_profile(&conn, "known-good", "known-good-64k").unwrap();
        assert_eq!(dup.name, "known-good-64k");
        assert_eq!(dup.ctx_size, 49152);
        assert_eq!(profile_origin(&conn, "known-good-64k").unwrap(), "auto");
        assert_eq!(
            get_profile(&conn, "known-good").unwrap().unwrap().ctx_size,
            49152,
            "source config untouched by the clone"
        );
        assert!(duplicate_profile(&conn, "known-good", "known-good-64k").is_err());
        assert!(duplicate_profile(&conn, "missing", "x").is_err());
    }
}
