use anyhow::Result;

pub fn get(key: Option<String>, json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    if let Some(k) = key {
        match deck_core::store::settings_get(&conn, &k)? {
            Some(v) => if json { println!("{v}") } else { println!("{k}={v}") },
            None => println!("{k} not set"),
        }
    } else {
        let rows = deck_core::store::settings_list(&conn)?;
        if rows.is_empty() { println!("no settings"); return Ok(()); }
        for (k,v,at) in rows { println!("{k}={v}  @{}", at); }
    }
    Ok(())
}

pub fn set(key: String, value: String, reason: String, actor: String) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    // allow raw string -> JSON string if not already JSON
    let val = if serde_json::from_str::<serde_json::Value>(&value).is_ok() { value } else { serde_json::Value::String(value.clone()).to_string() };
    deck_core::store::settings_set(&conn, &key, &val, &actor, &reason)?;
    println!("set {key}={val}");
    Ok(())
}

pub fn log(limit: usize) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let rows = deck_core::store::audit_list(&conn, limit)?;
    for (id, ts, actor, key, old, new, reason) in rows {
        println!("#{id} {ts} {actor} {key} {old:?} -> {new:?}  ({reason})");
    }
    Ok(())
}

pub fn undo(id: i64) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::settings_undo(&conn, id)?;
    println!("undid #{id}");
    Ok(())
}
