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
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> Result<FitRow, String> {
    deck_tauri::fit(
        PathBuf::from(model),
        ctx,
        kv_bytes,
        n_gpu_layers,
        kv_layers,
        reserve,
        offload,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_profile(profile: deck_tauri::Profile) -> Result<(), String> {
    deck_tauri::save_profile(profile).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_profile(name: String) -> Result<(), String> {
    deck_tauri::delete_profile(&name).map_err(|e| e.to_string())
}

#[tauri::command]
fn render_profile_unit(profile: deck_tauri::Profile) -> String {
    deck_tauri::render_profile_unit(profile)
}

#[tauri::command]
fn test_loadout(
    profile: deck_tauri::Profile,
    test_port: u16,
    app: tauri::AppHandle,
) -> Result<(), String> {
    deck_tauri::test_profile(&app, profile, test_port).map_err(|e| e.to_string())
}

#[tauri::command]
fn test_stop() -> Result<(), String> {
    deck_tauri::test_stop().map_err(|e| e.to_string())
}

#[tauri::command]
fn use_profile(name: String, dry_run: bool, managed: bool) -> Result<UseResult, String> {
    deck_tauri::use_profile(&name, dry_run, managed).map_err(|e| e.to_string())
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

#[tauri::command]
fn market_search(query: String, limit: usize) -> Result<Vec<deck_tauri::MarketHit>, String> {
    deck_tauri::market_search(&query, limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn market_files(repo_id: String) -> Result<Vec<deck_tauri::MarketFileRow>, String> {
    deck_tauri::market_files(&repo_id).map_err(|e| e.to_string())
}

#[tauri::command]
fn market_download(repo_id: String, rfilename: String) -> Result<String, String> {
    deck_tauri::market_download(&repo_id, &rfilename).map_err(|e| e.to_string())
}

#[tauri::command]
fn bench_now(
    engine: String,
    host: String,
    port: u16,
    model: String,
    ctx: u32,
) -> Result<deck_tauri::BenchRow, String> {
    deck_tauri::bench_now(&engine, &host, port, &model, ctx).map_err(|e| e.to_string())
}

#[tauri::command]
fn bench_history() -> Result<Vec<deck_tauri::BenchRow>, String> {
    deck_tauri::bench_history().map_err(|e| e.to_string())
}

#[tauri::command]
fn engine_status(engine: String, host: String, port: u16) -> deck_tauri::EngineStatus {
    deck_tauri::engine_status(&engine, &host, port)
}

#[tauri::command]
fn opencode_run(
    prompt: String,
    dir: String,
    auto: bool,
    model: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let model_opt = if model.is_empty() { None } else { Some(model.as_str()) };
    deck_tauri::opencode_run(&app, &prompt, &dir, auto, model_opt).map_err(|e| e.to_string())
}

#[tauri::command]
fn opencode_stop(id: String) -> Result<(), String> {
    deck_tauri::opencode_stop(&id).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            scan,
            list_models,
            list_profiles,
            dedup,
            fit,
            save_profile,
            delete_profile,
            render_profile_unit,
            test_loadout,
            test_stop,
            use_profile,
            signals_check,
            watchlist,
            watch_add,
            watch_remove,
            market_search,
            market_files,
            market_download,
            bench_now,
            bench_history,
            engine_status,
            opencode_run,
            opencode_stop
        ])
        .run(tauri::generate_context!())
        .expect("error while running cyberdeck");
}
