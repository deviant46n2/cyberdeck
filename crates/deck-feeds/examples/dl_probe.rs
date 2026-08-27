//! Manual transport probe for the download pipeline (not part of CI):
//!   timeout 90 cargo run -p deck-feeds --example dl_probe
use deck_feeds::{Cancel, download_file_progress, model_files};

fn main() {
    println!("[1] listing files…");
    let files = model_files("ggml-org/models").expect("model_files");
    println!("    {} downloadable file(s):", files.len());
    for f in &files {
        println!("      {} ({:?})", f.rfilename, f.size);
    }

    println!("[2] streaming tiny gguf…");
    let dest = std::env::temp_dir().join("deck-dl-probe");
    let cancel = Cancel::new();
    let start = std::time::Instant::now();
    let mut ticks = 0usize;
    let mut progress = |done: u64, total: u64| {
        ticks += 1;
        if ticks % 16 == 0 || done == total {
            eprintln!("    … {done} / {total} bytes");
        }
    };
    let expected = deck_feeds::remote_file_size("ggml-org/models", "tinyllamas/stories260K.gguf");
    let path = download_file_progress(
        "ggml-org/models",
        "tinyllamas/stories260K.gguf",
        &dest,
        expected,
        &mut progress,
        &cancel,
    )
    .expect("download");
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "[3] OK {path:?} in {:.2}s ({} bytes)",
        start.elapsed().as_secs_f32(),
        size
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(dest);
}
