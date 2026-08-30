//! Running a generation task against a live engine over its HTTP protocol.
//!
//! This is what makes the bench matrix engine-agnostic: a cell is
//! (model file × engine), and every engine speaks one of the registered
//! protocols (`EngineProtocol`). Adding a runtime you haven't heard of yet =
//! one protocol arm here.
//!
//! The sample always records the RAW ingredients (prompt/gen token counts,
//! wall time) so downstream math can recompute any derived metric; `tok_s`
//! additionally prefers a native generation-speed number when the engine
//! reports one (llama.cpp `timings`, Ollama `eval_duration`).

use std::time::Instant;

use deck_core::profile::{Engine, EngineProtocol};

/// One recorded generation against one engine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GenSample {
    pub ok: bool,
    pub text: String,
    pub prompt_tokens: Option<u64>,
    pub gen_tokens: Option<u64>,
    /// Generation tokens/sec: native when the engine reports it, else wall-based.
    pub tok_s: Option<f64>,
    /// How `tok_s` was obtained: `native` (engine timing) or `wall` (tokens /
    /// total request time — includes prompt prefill).
    pub tok_s_kind: &'static str,
    pub wall_ms: u64,
    pub error: Option<String>,
    // Phase 1 enriched — None when engine doesn't report
    pub prompt_tps: Option<f64>,
    pub ttft_ms: Option<u64>,
}

fn agent() -> ureq::Agent {
    // Generous: a long sequence of gen tokens can take minutes on a big MoE.
    let config = ureq::config::Config::builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .build();
    config.new_agent()
}

fn wall_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

fn sample_failed(start: Instant, text: &str) -> GenSample {
    GenSample {
        ok: false,
        text: String::new(),
        prompt_tokens: None,
        gen_tokens: None,
        tok_s: None,
        tok_s_kind: "native",
        wall_ms: wall_ms(start),
        error: Some(text.to_string()),
        prompt_tps: None,
        ttft_ms: None,
    }
}

/// POST a JSON body and return the response text (ureq 3 API).
fn post_json(url: &str, body: &serde_json::Value) -> Result<String, String> {
    let resp = agent()
        .post(url)
        .header("Content-Type", "application/json")
        .send_json(body)
        .map_err(|e| format!("request failed: {e}"))?;
    resp.into_body()
        .read_to_string()
        .map_err(|e| format!("reading response: {e}"))
}

/// Run one task prompt against a live engine and record the sample.
pub fn run_prompt(
    engine: Engine,
    host: &str,
    port: u16,
    model_id: &str,
    prompt: &str,
    max_tokens: u32,
) -> GenSample {
    match engine.protocol() {
        EngineProtocol::OpenAiChat => run_openai_chat(host, port, model_id, prompt, max_tokens),
        EngineProtocol::OllamaChat => run_ollama_chat(host, port, model_id, prompt, max_tokens),
    }
}

fn run_openai_chat(
    host: &str,
    port: u16,
    model_id: &str,
    prompt: &str,
    max_tokens: u32,
) -> GenSample {
    let url = format!("http://{host}:{port}/v1/chat/completions");
    let body = serde_json::json!({
        "model": model_id,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0,
        "stream": false,
    });
    let start = Instant::now();
    let text = match post_json(&url, &body) {
        Ok(t) => t,
        Err(e) => return sample_failed(start, &e),
    };
    let wall = wall_ms(start);
    let j: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return sample_failed(start, &format!("parse response: {e} — {text:?}")),
    };
    sample_from_openai_json(&j, wall)
}

/// Extract the raw ingredients from an OpenAI-compatible completion response.
fn sample_from_openai_json(j: &serde_json::Value, wall: u64) -> GenSample {
    let gen_tokens = j
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64());
    // llama.cpp injects `timings.predicted_per_second` into non-streamed
    // responses; FreeToken's OpenAI surface does not, so we fall back to a
    // wall-based number (kept honest via tok_s_kind).
    let native = j
        .pointer("/timings/predicted_per_second")
        .and_then(|v| v.as_f64());
    let (tok_s, tok_s_kind) = match native {
        Some(v) => (Some(v), "native"),
        None => match (gen_tokens, wall) {
            (Some(g), w) if w > 0 => (Some(g as f64 / w as f64 * 1000.0), "wall"),
            _ => (None, "wall"),
        },
    };
    // prompt throughput when timings present
    let prompt_tps = j
        .pointer("/timings/prompt_per_second")
        .and_then(|v| v.as_f64())
        .or_else(|| {
            let n = j.pointer("/timings/prompt_n").and_then(|v| v.as_u64())?;
            let ms = j.pointer("/timings/prompt_ms").and_then(|v| v.as_f64())?;
            if ms > 0.0 { Some(n as f64 / (ms / 1000.0)) } else { None }
        });
    GenSample {
        ok: true,
        text: j
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
        prompt_tokens: j.pointer("/usage/prompt_tokens").and_then(|v| v.as_u64()),
        gen_tokens,
        tok_s,
        tok_s_kind,
        wall_ms: wall,
        error: None,
        prompt_tps,
        ttft_ms: None, // streaming TTFT needs SSE; non-streamed endpoint can't report it
    }
}

fn run_ollama_chat(
    host: &str,
    port: u16,
    model_id: &str,
    prompt: &str,
    max_tokens: u32,
) -> GenSample {
    let url = format!("http://{host}:{port}/api/chat");
    let body = serde_json::json!({
        "model": model_id,
        "messages": [{"role": "user", "content": prompt}],
        "stream": false,
        "options": { "temperature": 0, "num_predict": max_tokens },
    });
    let start = Instant::now();
    let text = match post_json(&url, &body) {
        Ok(t) => t,
        Err(e) => return sample_failed(start, &e),
    };
    let wall = wall_ms(start);
    let j: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return sample_failed(start, &format!("parse response: {e} — {text:?}")),
    };
    sample_from_ollama_json(&j, wall)
}

/// Extract the raw ingredients from an Ollama `/api/chat` response.
fn sample_from_ollama_json(j: &serde_json::Value, wall: u64) -> GenSample {
    let gen_tokens = j.pointer("/eval_count").and_then(|v| v.as_u64());
    // tok/s = eval_count / (eval_duration_ns / 1e9).
    let eval_ns = j.pointer("/eval_duration").and_then(|v| v.as_u64());
    let tok_s = match (gen_tokens, eval_ns) {
        (Some(g), Some(ns)) if ns > 0 => Some(g as f64 / (ns as f64 / 1e9)),
        _ => match (gen_tokens, wall) {
            (Some(g), w) if w > 0 => Some(g as f64 / w as f64 * 1000.0),
            _ => None,
        },
    };
    let prompt_tps = j
        .pointer("/prompt_eval_count")
        .and_then(|v| v.as_u64())
        .zip(j.pointer("/prompt_eval_duration").and_then(|v| v.as_u64()))
        .and_then(|(c, ns)| if ns > 0 { Some(c as f64 / (ns as f64 / 1e9)) } else { None });
    GenSample {
        ok: true,
        text: j
            .pointer("/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
        prompt_tokens: j.pointer("/prompt_eval_count").and_then(|v| v.as_u64()),
        gen_tokens,
        tok_s,
        tok_s_kind: if eval_ns.unwrap_or(0) > 0 {
            "native"
        } else {
            "wall"
        },
        wall_ms: wall,
        error: None,
        prompt_tps,
        ttft_ms: j.pointer("/load_duration").and_then(|v| v.as_u64()).map(|ns| ns / 1_000_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_uses_native_llamacpp_timing() {
        let j = json!({
            "choices": [{"message": {"content": "H"}}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 40},
            "timings": {"predicted_per_second": 51.5},
        });
        let s = sample_from_openai_json(&j, 3000);
        assert!(s.ok);
        assert_eq!(s.prompt_tokens, Some(12));
        assert_eq!(s.gen_tokens, Some(40));
        assert_eq!(s.tok_s, Some(51.5));
        assert_eq!(s.tok_s_kind, "native");
        assert_eq!(s.text, "H");
    }

    #[test]
    fn openai_no_timing_falls_back_to_wall() {
        // FreeToken-style response: no `timings`.
        let j = json!({
            "choices": [{"message": {"content": "o"}}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 40},
        });
        let s = sample_from_openai_json(&j, 2000);
        assert_eq!(s.tok_s_kind, "wall");
        assert_eq!(s.tok_s.unwrap(), 20.0, "40 tok / 2s = 20 tok/s");
    }

    #[test]
    fn ollama_uses_eval_duration_for_native_speed() {
        let j = json!({
            "message": {"content": "y"},
            "prompt_eval_count": 3,
            "eval_count": 88,
            "eval_duration": 2_200_000_000_i64,
        });
        let s = sample_from_ollama_json(&j, 999_999);
        assert_eq!(s.prompt_tokens, Some(3));
        assert_eq!(s.gen_tokens, Some(88));
        assert_eq!(s.tok_s.unwrap(), 40.0, "88 tok / 2.2s = 40 tok/s");
        assert_eq!(s.tok_s_kind, "native");
    }
}
