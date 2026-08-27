//! Prints parsed GGUF metadata without touching tensor data.
//!
//! Usage: cargo run -p deck-core --example dump_meta -- /path/to/model.gguf

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: dump_meta <model.gguf>");
        return ExitCode::from(2);
    };

    match deck_core::gguf::GgufMeta::read(&path) {
        Ok(meta) => {
            println!("file      : {path}");
            println!("size      : {:.2} GiB", meta.file_size as f64 / 1073741824.0);
            println!("gguf ver  : {}", meta.version);
            println!("tensors   : {}", meta.tensor_count);
            println!("arch      : {:?}", meta.arch().unwrap_or("?"));
            println!("name      : {:?}", meta.name().unwrap_or("?"));
            println!(
                "quant     : {:?}",
                meta.quant_name().unwrap_or_else(|| "?".into())
            );
            println!("ctx_train : {:?}", meta.ctx_train());
            println!("vocab     : {:?}", meta.vocab_size());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
