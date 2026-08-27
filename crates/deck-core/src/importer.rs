//! Imports existing launch scripts into loadouts.
//!
//! Seeds cyberdeck's profile store from the hand-written wrappers you already
//! run, so the tool starts with a faithful copy of today's known-good config.
//! Handles the common pattern where the model/binary are shell variables
//! (`MODEL=/path` ... `-m "$MODEL"`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::profile::{Engine, Profile};

/// Splits script text into tokens, keeping quoted spans intact.
fn tokenize(text: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in text.chars() {
        if c == '"' {
            in_q = !in_q;
            cur.push(c);
        } else if c.is_whitespace() && !in_q {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

/// Extracts a flag's value from the command body. Matches `--flag`, `-flag`,
/// or `--flag=value`, treating flags as whole tokens (so `-m` won't match
/// inside `--load-mode`).
fn extract(text: &str, flag: &str) -> Option<String> {
    let toks = tokenize(text);
    for i in 0..toks.len() {
        for pat in [format!("--{flag}"), format!("-{flag}")] {
            let t = &toks[i];
            if *t == pat {
                if i + 1 < toks.len() {
                    return Some(unquote(&toks[i + 1]));
                }
            } else if let Some(v) = t.strip_prefix(&format!("{pat}=")) {
                return Some(unquote(v));
            }
        }
    }
    None
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').trim_matches('\'').to_string()
}

/// Collects `VAR=value` assignments anywhere in the script (pre-exec vars).
fn parse_vars(text: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or(""); // drop trailing comment
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() && k.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
                vars.insert(k.to_string(), unquote(v));
            }
        }
    }
    vars
}

/// Resolves a `$VAR` reference against collected assignments.
fn resolve(vars: &HashMap<String, String>, token: &str) -> String {
    if let Some(name) = token.strip_prefix('$') {
        vars.get(name).cloned().unwrap_or_else(|| token.to_string())
    } else {
        token.to_string()
    }
}

fn parse_common(p: &mut Profile, text: &str) {
    if let Some(v) = extract(text, "alias") {
        p.alias = v;
    }
    if let Some(v) = extract(text, "ctx-size") {
        if let Ok(n) = v.parse() {
            p.ctx_size = n;
        }
    }
    if let Some(v) = extract(text, "n-gpu-layers") {
        if let Ok(n) = v.parse() {
            p.n_gpu_layers = n;
        }
    }
    if let Some(v) = extract(text, "ubatch-size") {
        if let Ok(n) = v.parse() {
            p.ubatch_size = n;
        }
    }
    if let Some(v) = extract(text, "port") {
        if let Ok(n) = v.parse() {
            p.port = n;
        }
    }
    if let Some(v) = extract(text, "host") {
        p.host = v;
    }
    if let Some(v) = extract(text, "temp") {
        if let Ok(n) = v.parse() {
            p.temperature = n;
        }
    }
    if let Some(v) = extract(text, "top-p") {
        if let Ok(n) = v.parse() {
            p.top_p = n;
        }
    }
    if let Some(v) = extract(text, "top-k") {
        if let Ok(n) = v.parse() {
            p.top_k = n;
        }
    }
    if let Some(v) = extract(text, "parallel") {
        if let Ok(n) = v.parse() {
            p.parallel = n;
        }
    }
    if let Some(v) = extract(text, "load-mode") {
        p.load_mode = Some(v);
    }
    if let Some(v) = extract(text, "cache-type-k") {
        p.kv_cache_type_k = Some(v);
    }
    if let Some(v) = extract(text, "cache-type-v") {
        p.kv_cache_type_v = Some(v);
    }
    if let Some(v) = extract(text, "flash-attn") {
        p.flash_attn = v == "on";
    }
    if let Some(v) = extract(text, "reasoning") {
        p.reasoning = Some(v);
    }
    if let Some(v) = extract(text, "reasoning-format") {
        p.reasoning_format = Some(v);
    }
    if let Some(v) = extract(text, "reasoning-effort") {
        p.reasoning_effort = Some(v);
    }
    if let Some(v) = extract(text, "reasoning-budget") {
        if let Ok(n) = v.parse() {
            p.reasoning_budget = Some(n);
        }
    }
}

pub fn import_llamacpp_script(path: impl AsRef<Path>, name: &str) -> Result<Profile> {
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.as_ref().display()))?;
    Ok(parse_llamacpp_script_text(&text, name))
}

/// Parses llama.cpp launch script text into a profile (no IO).
pub fn parse_llamacpp_script_text(text: &str, name: &str) -> Profile {
    let vars = parse_vars(text);
    let mut p = Profile::default();
    p.name = name.to_string();
    p.engine = Engine::LlamaCpp;

    // binary: the token right after `exec "..."`
    if let Some(bin) = text.split("exec").nth(1).and_then(|s| s.split('"').nth(1)) {
        p.bin = std::path::PathBuf::from(resolve(&vars, bin));
    }
    // model + flags live in the command body (after `exec`), away from comments
    let body = text.split("exec").nth(1).unwrap_or(&text);
    if let Some(m) = extract(body, "m") {
        p.model = resolve(&vars, &m);
    }
    parse_common(&mut p, body);
    p
}

pub fn import_freetoken_script(path: impl AsRef<Path>, name: &str) -> Result<Profile> {
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.as_ref().display()))?;
    Ok(parse_freetoken_script_text(&text, name))
}

/// Parses FreeToken launch script text into a profile (no IO).
pub fn parse_freetoken_script_text(text: &str, name: &str) -> Profile {
    let vars = parse_vars(text);
    let mut p = Profile::default();
    p.name = name.to_string();
    p.engine = Engine::FreeToken;

    let body = text.split("ft serve").nth(1).unwrap_or(&text);
    if let Some(model) = body.split("--model").nth(1) {
        p.model = resolve(&vars, &unquote(model.split_whitespace().next().unwrap_or("")));
    }
    if let Some(bin) = text.split("exec").nth(1).and_then(|s| s.split('"').nth(1)) {
        p.bin = std::path::PathBuf::from(resolve(&vars, bin));
    }
    parse_common(&mut p, body);

    if let Some(v) = extract(&text, "moe-backend") {
        p.ft_backend = Some(v);
    }
    if let Some(v) = extract(&text, "moe-cache-size") {
        if let Ok(n) = v.parse() {
            p.ft_moe_cache_size = Some(n);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"
# header comment with --load-mode mmap+mlock should NOT confuse parsing
MODEL=/models/x.gguf
BIN=/bin/llama-server
exec "$BIN" \
  -m "$MODEL" \
  --alias qwen \
  --ctx-size 65536 \
  --n-gpu-layers 64 \
  --flash-attn on \
  --reasoning on
"#;

    #[test]
    fn resolves_vars_and_flags() {
        let p = parse_llamacpp_script_text(SCRIPT, "t");
        assert_eq!(p.model, "/models/x.gguf");
        assert_eq!(p.bin, PathBuf::from("/bin/llama-server"));
        assert_eq!(p.alias, "qwen");
        assert_eq!(p.ctx_size, 65536);
        assert_eq!(p.n_gpu_layers, 64);
        assert!(p.flash_attn);
    }

    #[test]
    fn does_not_match_flag_inside_word() {
        // --load-mode in a comment must not be read as -m / --m
        let p = parse_llamacpp_script_text(SCRIPT, "t");
        assert_eq!(p.model, "/models/x.gguf"); // not "mmap+mlock" or similar
        assert_eq!(p.load_mode, Some("mmap+mlock".into()));
    }

    #[test]
    fn freetoken_parses_offload() {
        let s = "exec ft serve --model nvidia/Qwen-X --port 1919 --moe-backend offload --moe-cache-size 3000";
        let p = parse_freetoken_script_text(s, "ft");
        assert_eq!(p.engine, Engine::FreeToken);
        assert_eq!(p.model, "nvidia/Qwen-X");
        assert_eq!(p.port, 1919);
        assert_eq!(p.ft_backend.as_deref(), Some("offload"));
        assert_eq!(p.ft_moe_cache_size, Some(3000));
    }
}
