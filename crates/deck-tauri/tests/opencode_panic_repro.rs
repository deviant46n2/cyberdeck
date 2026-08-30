#[test]
fn repro_opencode_run() {
    let app = tauri::test::mock_app();
    let handle = app.handle();
    deck_tauri::console::opencode_run(
        &handle,
        "testing",
        "/home/deviant/Projects/cyberdeck",
        false,
        deck_tauri::console::Engine::LlamaCpp,
        Some("llamacpp/qwen3.8-27b"),
    )
    .expect("opencode_run should not panic");
    std::thread::sleep(std::time::Duration::from_secs(3));
}
