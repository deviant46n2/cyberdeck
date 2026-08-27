// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use deck_tauri::{DupRow, FitRow, ModelRow, ProfileRow, ScanResult, UseResult};
use std::path::PathBuf;

#[tauri::command]
fn scan() -> Result<ScanResult, String> {
    deck_tauri::scan().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_models() -> Result<Vec<ModelRow>, String> {
    deck_tauri::list_models().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_profiles() -> Result<Vec<ProfileRow>, String> {
    deck_tauri::list_profiles().map_err(|e| e.to_string())
}

#[tauri::command]
fn dedup() -> Result<Vec<DupRow>, String> {
    deck_tauri::dedup().map_err(|e| e.to_string())
}

#[tauri::command]
fn fit(
    model: String,
    ctx: u32,
    kv_bytes: f64,
    ngl: f64,
    kv_layers: Option<u64>,
    reserve: u64,
) -> Result<FitRow, String> {
    deck_tauri::fit(PathBuf::from(model), ctx, kv_bytes, ngl, kv_layers, reserve)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn use_profile(name: String, dry_run: bool) -> Result<UseResult, String> {
    deck_tauri::use_profile(&name, dry_run).map_err(|e| e.to_string())
}

#[tauri::command]
fn signals_check(limit: usize) -> Result<Vec<deck_tauri::SignalRow>, String> {
    deck_tauri::signals_check(limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn watchlist() -> Result<Vec<String>, String> {
    deck_tauri::watchlist().map_err(|e| e.to_string())
}

#[tauri::command]
fn watch_add(org: String) -> Result<(), String> {
    deck_tauri::watch_add(&org).map_err(|e| e.to_string())
}

#[tauri::command]
fn watch_remove(org: String) -> Result<(), String> {
    deck_tauri::watch_remove(&org).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            scan,
            list_models,
            list_profiles,
            dedup,
            fit,
            use_profile,
            signals_check,
            watchlist,
            watch_add,
            watch_remove
        ])
        .run(tauri::generate_context!())
        .expect("error while running cyberdeck");
}
