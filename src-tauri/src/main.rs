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

/// Full loadout for the EDITOR. The list command returns summary rows that
/// lack editor fields (model/ngl/ctx_ladder/caches); editing needs the real
/// profile or the ADVANCED panel crashes on the first missing array.
#[tauri::command]
fn profile_get(name: String) -> Result<Option<deck_tauri::Profile>, String> {
    deck_tauri::get_profile(&name).map_err(|e| e.to_string())
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

/// Authoritative state for keys — the frontend reconciles the store against
/// this after enqueueing so a dropped `dl-start`/`dl-done` event can't leave
/// a row pinned in `queued` forever.
#[tauri::command]
fn download_states(keys: Vec<String>) -> Vec<deck_tauri::DownloadState> {
    deck_tauri::download_states_json(&keys)
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
async fn apply_cached_profile(
    profile: deck_tauri::Profile,
    fit: Option<deck_tauri::FitBreakdown>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    deck_tauri::apply_cached_profile(&app, profile, fit).map_err(|e| e.to_string())
}

/// One-click TEST/LOAD straight from a MARKET repo file: download (shared
/// queue, resume-aware) → derive → verify → [apply → bench]. Streams the
/// same `bringup-*` events as the local-file pipeline.
#[tauri::command]
async fn experiment_start(
    repo_id: String,
    rfilename: String,
    engine: String,
    apply: bool,
    app: tauri::AppHandle,
) -> Result<(), String> {
    deck_tauri::experiment_start(&app, &repo_id, &rfilename, &engine, apply)
        .map_err(|e| e.to_string())
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

/// Run the blind A/B compare grid headlessly (long-running; off the UI thread)
/// and return the scored report for the Compare tab.
#[tauri::command]
async fn compare_run(
    model: String,
    engines: Vec<String>,
    ollama: Vec<String>,
    tasks: Vec<String>,
    runs: u32,
    max_tokens: u32,
    seed: u64,
) -> Result<deck_tauri::CompareReport, String> {
    blocking(move || {
        deck_tauri::compare_run(model, engines, ollama, tasks, runs, max_tokens, seed)
    })
    .await
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
    engine: String,
    ctx: u32,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let model_opt = if model.is_empty() {
            None
        } else {
            Some(model.as_str())
        };
        let engine_opt = if engine.is_empty() {
            deck_tauri::console::Engine::LlamaCpp
        } else {
            match engine.as_str() {
                "freetoken" => deck_tauri::console::Engine::FreeToken,
                "ollama" => deck_tauri::console::Engine::Ollama,
                _ => deck_tauri::console::Engine::LlamaCpp,
            }
        };
        deck_tauri::opencode_run(&app, &prompt, &dir, auto, engine_opt, model_opt, ctx).map_err(|e| e.to_string())
    }));
    Ok(())
}

#[tauri::command]
fn opencode_stop(id: String) -> Result<(), String> {
    deck_tauri::opencode_stop(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn tui_spawn(dir: String, cols: u16, rows: u16, app: tauri::AppHandle) -> Result<String, String> {
    deck_tauri::tui_spawn(&app, &dir, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
fn tui_write(id: String, bytes: Vec<u8>) -> Result<(), String> {
    deck_tauri::tui_write(&id, &bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn tui_resize(id: String, cols: u16, rows: u16) -> Result<(), String> {
    deck_tauri::tui_resize(&id, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
fn tui_stop(id: String) -> Result<(), String> {
    deck_tauri::tui_stop(&id).map_err(|e| e.to_string())
}

#[tauri::command]
fn hw_info() -> deck_tauri::HwInfo {
    deck_tauri::hw_info()
}

#[tauri::command]
fn host_metrics() -> deck_tauri::LiveMetrics {
    deck_tauri::host_metrics()
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
fn engine_list() -> Vec<deck_tauri::EngineDescriptor> {
    deck_tauri::engine_list()
}

/// Live PORT MAP for the UI: each engine's fixed slot, bound profile, resident
/// flag, and up/down state. Probes are non-blocking one-shot checks.
#[tauri::command]
async fn port_map_status(host: String) -> Vec<deck_tauri::PortMapSlot> {
    tauri::async_runtime::spawn_blocking(move || deck_tauri::port_map_status(&host))
        .await
        .unwrap_or_default()
}

/// Stop one engine's unit and clear its port-map binding (UI door to
/// `deck engines stop`); other residents are untouched.
#[tauri::command]
async fn engine_stop(engine: String) -> Result<(), String> {
    blocking(move || deck_tauri::engine_stop(&engine).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn engine_bin_list() -> Vec<deck_tauri::EngineBinRow> {
    deck_tauri::engine_bin_list()
}

#[tauri::command]
async fn engine_bin_set(store_id: String, bin: String) -> Result<(), String> {
    blocking(move || deck_tauri::engine_bin_set(&store_id, &bin).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn engine_bin_clear(store_id: String) -> Result<(), String> {
    blocking(move || deck_tauri::engine_bin_clear(&store_id).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn feeds_poll(sources: Vec<String>) -> Result<deck_tauri::FeedsPollResult, String> {
    blocking(move || deck_tauri::feeds_poll(sources).map_err(|e| e.to_string())).await
}

#[tauri::command]
fn feeds_list(limit: usize) -> Result<Vec<deck_tauri::Release>, String> {
    deck_tauri::feeds_list(limit).map_err(|e| e.to_string())
}

#[tauri::command]
fn feeds_rank(limit: usize, workload: Option<String>) -> Result<Vec<deck_tauri::RankedRelease>, String> {
    deck_tauri::feeds_rank(limit, workload).map_err(|e| e.to_string())
}

#[tauri::command]
fn engine_start(engine: String) -> Result<(), String> {
    deck_tauri::engine_start(&engine).map_err(|e| e.to_string())
}

#[tauri::command]
fn bringup_reset() -> Result<(), String> {
    deck_tauri::bringup_reset();
    Ok(())
}

#[tauri::command]
fn workloads_list() -> Result<Vec<deck_tauri::Workload>, String> {
    deck_tauri::workloads_list().map_err(|e| e.to_string())
}

#[tauri::command]
fn hardware_profile() -> Result<deck_core::hardware::HardwareProfile, String> {
    deck_tauri::hardware_profile().map_err(|e| e.to_string())
}

#[tauri::command]
fn recommend(workload: String, objective: String) -> Result<Vec<deck_core::recommend::RankedCandidate>, String> {
    deck_tauri::recommend(workload, objective).map_err(|e| e.to_string())
}

#[tauri::command]
fn settings_get(key: String) -> Result<Option<String>, String> {
    deck_tauri::settings_get(&key).map_err(|e| e.to_string())
}

#[tauri::command]
fn settings_set(key: String, value: String, reason: String, actor: String) -> Result<(), String> {
    deck_tauri::settings_set(key, value, reason, actor).map_err(|e| e.to_string())
}

#[tauri::command]
fn settings_list() -> Result<Vec<(String, String, i64)>, String> {
    deck_tauri::settings_list().map_err(|e| e.to_string())
}

#[tauri::command]
fn agent_tools() -> Vec<deck_tauri::agent::ToolDef> {
    deck_tauri::agent_tools()
}

#[tauri::command]
fn analyze_relevance(repo: String, workload: Option<String>) -> Result<serde_json::Value, String> {
    deck_tauri::analyze_relevance(repo, workload)
}

#[tauri::command]
fn delete_model(path: String, delete_file: bool) -> Result<deck_tauri::DeleteResult, String> {
    deck_tauri::delete_model(&path, delete_file).map_err(|e| e.to_string())
}

#[tauri::command]
fn dedup_delete(identity: String, delete_file: bool) -> Result<usize, String> {
    deck_tauri::dedup_delete(&identity, delete_file).map_err(|e| e.to_string())
}

#[tauri::command]
async fn workflow_seed() -> Result<String, String> {
    blocking(move || deck_tauri::workflow_seed().map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn workflow_save(body: String) -> Result<String, String> {
    blocking(move || deck_tauri::workflow_save(&body).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn workflow_list() -> Result<Vec<deck_core::workflow::Workflow>, String> {
    blocking(move || deck_tauri::workflow_list().map_err(|e| e.to_string())).await
}

#[tauri::command]
fn workflow_run(
    workflow_id: String,
    runner: String,
    dir: Option<String>,
    model: Option<String>,
    task: Option<String>,
    app: tauri::AppHandle,
) -> Result<deck_tauri::WfStarted, String> {
    deck_tauri::workflow_run(&app, &workflow_id, &runner, dir.as_deref(), model.as_deref(), task.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn workflow_stop(run_id: String) -> Result<(), String> {
    deck_tauri::workflow_stop(&run_id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn workflow_history(
    workflow_id: Option<String>,
) -> Result<Vec<deck_core::wfstore::WorkflowRunRow>, String> {
    blocking(move || deck_tauri::workflow_history(workflow_id.as_deref()).map_err(|e| e.to_string()))
        .await
}

#[tauri::command]
async fn workflow_get(
    workflow_id: String,
) -> Result<Option<deck_core::workflow::Workflow>, String> {
    blocking(move || deck_tauri::workflow_get(&workflow_id).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn workflow_per_role_bench(
    workflow_id: String,
) -> Result<Vec<deck_core::store::RoleBenchRow>, String> {
    blocking(move || {
        deck_tauri::workflow_per_role_bench(&workflow_id).map_err(|e| e.to_string())
    })
    .await
}

#[tauri::command]
async fn workflow_loop_bench(
    workflow_id: String,
) -> Result<Option<deck_core::store::LoopBenchRow>, String> {
    blocking(move || deck_tauri::workflow_loop_bench(&workflow_id).map_err(|e| e.to_string())).await
}

fn main() {
    // NVIDIA + Wayland: WebKitGTK's GBM/DMA-BUF scanout path fails with
    // "Failed to create GBM buffer ... Invalid argument" and the window stays
    // black. Force the software compositing fallback before WebKit spawns.
    unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
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
            profile_get,
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
            download_states,
            download_cancel,
            download_remove,
            index_downloaded,
            bringup_start,
            test_model_start,
            apply_cached_profile,
            experiment_start,
            bench_now,
            bench_history,
            compare_run,
            engine_status,
            opencode_run,
            opencode_stop,
            tui_spawn,
            tui_write,
            tui_resize,
            tui_stop,
            hw_info,
            host_metrics,
            browse_fit_remote,
            tweak_profile,
            engine_list,
            engine_bin_list,
            engine_bin_set,
            engine_bin_clear,
            port_map_status,
            engine_stop,
            feeds_poll,
            feeds_list,
            feeds_rank,
            engine_start,
            bringup_reset,
            workloads_list,
            hardware_profile,
            recommend,
            settings_get,
            settings_set,
            settings_list,
            agent_tools,
            analyze_relevance,
            workflow_seed,
            workflow_save,
            workflow_list,
            workflow_get,
            workflow_run,
            workflow_stop,
            workflow_history,
            workflow_per_role_bench,
            workflow_loop_bench
        ])
        .setup(|_app| {
            // Sweep agents orphaned by a previously crashed/crash-killed app
            // instance; a live owner chain means they are still owned and skipped.
            deck_tauri::reap_orphans();
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building cyberdeck")
        .run(|_app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                deck_tauri::kill_all();
            }
        });
}
