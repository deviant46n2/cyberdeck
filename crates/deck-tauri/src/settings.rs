pub fn settings_get(key: &str) -> Result<Option<String>, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::settings_get(&conn, key).map_err(|e| e.to_string())
}

pub fn settings_set(key: String, value: String, reason: String, actor: String) -> Result<(), String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    let val = if serde_json::from_str::<serde_json::Value>(&value).is_ok() { value.clone() } else { serde_json::Value::String(value.clone()).to_string() };
    deck_core::store::settings_set(&conn, &key, &val, &actor, &reason).map_err(|e| e.to_string())
}

pub fn settings_list() -> Result<Vec<(String, String, i64)>, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::settings_list(&conn).map_err(|e| e.to_string())
}
