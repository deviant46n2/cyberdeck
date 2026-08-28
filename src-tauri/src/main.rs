// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Sync Tauri commands execute on the MAIN THREAD, so any command that touches
// the network (curl/HF) or heavy FS/sqlite work must run via spawn_blocking or
// it freezes the window. Everything remote is async below; fast local SQLite
// reads stay sync on purpose.
use deck_tauri::{DupRow, FitRow, ModelRow, ProfileRow, ScanResult, TweakResult, UseResult};
use std::path::PathBuf;
use tauri::Emitter;

async fn blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("join failed: {e}"))?
}

/// Like scan() but emits a `scan_done` event so the Downloads drawer can show
/// "N models indexed" feedback after a download completes.
#[tauri::command]
async fn scan_with_event(app: tauri::AppHandle) -> Result<ScanResult, String> {
    let r = blocking(move || deck_tauri::scan().map_err(|e| e.to_string())).await?;
    let _ = app.emit(
        "scan_done",
        serde_json::json!({ "indexed": r.indexed, "dups": r.dups.len() }),
    );
    Ok(r)
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
async fn fit(
    model: String,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> Result<FitRow, String> {
    blocking(move || {
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
    })
    .await
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
async fn signals_check(limit: usize) -> Result<Vec<deck_tauri::SignalRow>, String> {
    blocking(move || deck_tauri::signals_check(limit).map_err(|e| e.to_string())).await
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
async fn market_search(query: String, limit: usize) -> Result<Vec<deck_tauri::MarketHit>, String> {
    blocking(move || deck_tauri::market_search(&query, limit).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn browse_org(org: String, limit: usize) -> Result<Vec<deck_tauri::MarketHit>, String> {
    blocking(move || deck_tauri::browse_org(&org, limit).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn market_files(repo_id: String) -> Result<Vec<deck_tauri::MarketFileRow>, String> {
    blocking(move || deck_tauri::market_files(&repo_id).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn download_start(
    repo_id: String,
    rfilename: String,
    app: tauri::AppHandle,
) -> Result<deck_tauri::DownloadStarted, String> {
    deck_tauri::download_start(&app, &repo_id, &rfilename).map_err(|e| e.to_string())
}

#[tauri::command]
fn download_cancel(key: String) -> Result<(), String> {
    deck_tauri::download_cancel(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn download_remove(key: String, rfilename: String) -> Result<(), String> {
    deck_tauri::download_remove(&key, &rfilename).map_err(|e| e.to_string())
}

#[tauri::command]
async fn index_downloaded(paths: Vec<String>) -> Result<usize, String> {
    blocking(move || deck_tauri::index_downloaded(&paths).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn bringup_start(
    model: String,
    engine: String,
    fast: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    deck_tauri::bringup_start(&app, &model, &engine, fast).map_err(|e| e.to_string())
}

#[tauri::command]
async fn test_model_start(
    model: String,
    engine: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    deck_tauri::test_model_start(&app, &model, &engine).map_err(|e| e.to_string())
}

#[tauri::command]
async fn bench_now(
    engine: String,
    host: String,
    port: u16,
    model: String,
    ctx: u32,
) -> Result<deck_tauri::BenchRow, String> {
    blocking(move || {
        deck_tauri::bench_now(&engine, &host, port, &model, ctx).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn bench_history() -> Result<Vec<deck_tauri::BenchRow>, String> {
    deck_tauri::bench_history().map_err(|e| e.to_string())
}

#[tauri::command]
async fn engine_status(engine: String, host: String, port: u16) -> deck_tauri::EngineStatus {
    let fallback = (engine.clone(), host.clone());
    tauri::async_runtime::spawn_blocking(move || deck_tauri::engine_status(&engine, &host, port))
        .await
        .unwrap_or(deck_tauri::EngineStatus {
            engine: fallback.0,
            host: fallback.1,
            port,
            up: false,
        })
}

#[tauri::command]
fn opencode_run(
    prompt: String,
    dir: String,
    auto: bool,
    model: String,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let model_opt = if model.is_empty() {
        None
    } else {
        Some(model.as_str())
    };
    deck_tauri::opencode_run(&app, &prompt, &dir, auto, model_opt).map_err(|e| e.to_string())
}

#[tauri::command]
fn opencode_stop(id: String) -> Result<(), String> {
    deck_tauri::opencode_stop(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn hw_info() -> deck_tauri::HwInfo {
    deck_tauri::hw_info()
}

#[tauri::command]
async fn browse_fit_remote(
    repo_id: String,
    rfilename: String,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> Result<deck_tauri::BrowseFitResult, String> {
    blocking(move || {
        deck_tauri::browse_fit_remote(
            &repo_id,
            &rfilename,
            ctx,
            kv_bytes,
            n_gpu_layers,
            kv_layers,
            reserve,
            offload,
        )
        .map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
fn tweak_profile(
    profile: deck_tauri::Profile,
    ctx_override: Option<u32>,
    kv_bytes_override: Option<f64>,
    offload_override: Option<bool>,
    ngl_override: Option<u32>,
) -> TweakResult {
    deck_tauri::test_profile_tweaked(
        profile,
        ctx_override,
        kv_bytes_override,
        offload_override,
        ngl_override,
    )
}

#[tauri::command]
fn delete_model(path: String, delete_file: bool) -> Result<usize, String> {
    deck_tauri::delete_model(&path, delete_file).map_err(|e| e.to_string())
}

#[tauri::command]
fn dedup_delete(identity: String, delete_file: bool) -> Result<usize, String> {
    deck_tauri::dedup_delete(&identity, delete_file).map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            scan_with_event,
            list_models,
            list_profiles,
            dedup,
            delete_model,
            dedup_delete,
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
            browse_org,
            market_files,
            download_start,
            download_cancel,
            download_remove,
            index_downloaded,
            bringup_start,
            test_model_start,
            bench_now,
            bench_history,
            engine_status,
            opencode_run,
            opencode_stop,
            hw_info,
            browse_fit_remote,
            tweak_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running cyberdeck");
}
