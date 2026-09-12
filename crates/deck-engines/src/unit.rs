//! Rendering the systemd unit file for an engine profile: argument building
//! and the final unit text. Pure — no touch of the running service.

use std::path::PathBuf;

use deck_core::profile::{Engine, Profile};
use deck_core::runtime::RuntimeManifest;

/// The runtime manifest bound to a profile, if it targets a custom runtime
/// (one added as a JSON manifest rather than an `Engine` variant).
pub fn manifest_for(p: &Profile) -> Option<RuntimeManifest> {
    let id = p.runtime_id.as_ref()?;
    deck_core::runtime::all_manifests()
        .into_iter()
        .find(|m| m.id == *id)
}

/// systemd unit name: the manifest's for a custom runtime, else the engine's.
pub fn unit_name_for(p: &Profile) -> String {
    manifest_for(p)
        .map(|m| m.unit_name)
        .unwrap_or_else(|| p.engine.systemd_unit().to_string())
}

/// Display name for a profile (manifest display for custom runtimes).
pub fn display_for(p: &Profile) -> String {
    manifest_for(p)
        .map(|m| m.display)
        .unwrap_or_else(|| p.engine.descriptor().display.to_string())
}

/// Env for a *headless test child* (test-port verification), which must NOT
/// carry the unit's auth var: llama-server would then require the API key on
/// the health probe and look dead. Builtin engines add only their bind var;
/// a custom runtime keeps its manifest env (the author's contract).
pub fn child_env_for(p: &Profile) -> Vec<(String, String)> {
    if let Some(m) = manifest_for(p) {
        return m.render_env(&p.model, &p.host, p.port, p.ctx_size, &p.alias, &p.params);
    }
    if p.engine == Engine::Ollama {
        return vec![("OLLAMA_HOST".into(), format!("{}:{}", p.host, p.port))];
    }
    Vec::new()
}

/// Launcher argv: the manifest template when a custom runtime is bound, else
/// the builtin engine's flags. This is the single dispatch point that keeps
/// runtime-specific knowledge out of every caller.
pub fn args_for(p: &Profile) -> Vec<String> {
    match manifest_for(p) {
        Some(m) => m.render_argv(&p.model, &p.host, p.port, p.ctx_size, &p.alias, &p.params),
        None => build_args(p),
    }
}

/// Environment for the unit/child process. Builtin engines add their auth/host
/// vars; a custom runtime's manifest `env` (with template substitution) wins.
pub fn env_for(p: &Profile) -> Vec<(String, String)> {
    if let Some(m) = manifest_for(p) {
        return m.render_env(&p.model, &p.host, p.port, p.ctx_size, &p.alias, &p.params);
    }
    let mut env = Vec::new();
    if p.engine == Engine::LlamaCpp || p.engine == Engine::UncensoredLlamaCpp {
        env.push(("LLAMACPP_API_KEY".into(), "llamacpp-local".into()));
    }
    if p.engine == Engine::Ollama {
        // Ollama binds where OLLAMA_HOST points; the port contract lives here,
        // not in ExecStart args (ollama serve takes no --host/--port flags).
        env.push(("OLLAMA_HOST".into(), format!("{}:{}", p.host, p.port)));
    }
    env
}

pub(crate) fn systemd_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("systemd/user")
}

pub(crate) fn generated_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("cyberdeck/generated")
}

/// Builds the `ExecStart` argument list for an engine from a profile.
pub fn build_args(p: &Profile) -> Vec<String> {
    match p.engine {
        Engine::LlamaCpp | Engine::UncensoredLlamaCpp => {
            let mut a = vec![
                "-m".into(),
                p.model.clone(),
                "--alias".into(),
                p.alias.clone(),
                "--ctx-size".into(),
                p.ctx_size.to_string(),
                "--n-gpu-layers".into(),
                p.n_gpu_layers.to_string(),
                "--ubatch-size".into(),
                p.ubatch_size.to_string(),
                "--parallel".into(),
                p.parallel.to_string(),
                "--temp".into(),
                p.temperature.to_string(),
                "--top-p".into(),
                p.top_p.to_string(),
                "--top-k".into(),
                p.top_k.to_string(),
                "--port".into(),
                p.port.to_string(),
                "--host".into(),
                p.host.clone(),
            ];
            if p.metrics {
                a.push("--metrics".into());
            }
            if p.flash_attn {
                a.push("--flash-attn".into());
                a.push("on".into());
            }
            if let Some(k) = &p.kv_cache_type_k {
                a.push("--cache-type-k".into());
                a.push(k.clone());
            }
            if let Some(v) = &p.kv_cache_type_v {
                a.push("--cache-type-v".into());
                a.push(v.clone());
            }
            if let Some(lm) = &p.load_mode {
                a.push("--load-mode".into());
                a.push(lm.clone());
            }
            if let Some(s) = &p.spec_type {
                a.push("--spec-type".into());
                a.push(s.clone());
            }
            if let Some(d) = &p.draft_model {
                a.push("--draft-model".into());
                a.push(d.display().to_string());
            }
            for (flag, val) in [
                ("reasoning", &p.reasoning),
                ("reasoning-format", &p.reasoning_format),
                ("reasoning-effort", &p.reasoning_effort),
            ] {
                if let Some(v) = val {
                    a.push(format!("--{flag}"));
                    a.push(v.clone());
                }
            }
            if let Some(b) = p.reasoning_budget {
                a.push("--reasoning-budget".into());
                a.push(b.to_string());
            }
            a
        }
        Engine::FreeToken => {
            let mut a = vec![
                "serve".into(),
                "--model".into(),
                p.model.clone(),
                "--host".into(),
                p.host.clone(),
                "--port".into(),
                p.port.to_string(),
            ];
            if let Some(b) = &p.ft_backend {
                a.push("--moe-backend".into());
                a.push(b.clone());
            }
            if let Some(c) = p.ft_moe_cache_size {
                a.push("--moe-cache-size".into());
                a.push(c.to_string());
            }
            a
        }
        Engine::Ollama => {
            // A daemon, not a per-model launch: no model path at exec time and
            // no host/port flags — Ollama binds via the OLLAMA_HOST env
            // rendered by `render_unit`. Models are addressed per-request by id
            // via /api/chat (see EngineProtocol::OllamaChat).
            vec!["serve".into()]
        }
    }
}

/// Renders the full systemd unit file content. Runtime-agnostic: the launcher
/// argv comes from `args_for` (builtin flags or a custom manifest template).
pub fn render_unit(p: &Profile) -> String {
    let args = args_for(p);
    let exec = format!("{} {}", p.bin.display(), shell_join(&args));
    let mut s = String::new();
    s.push_str(&format!(
        "# generated by cyberdeck — profile '{}'\n",
        p.name
    ));
    s.push_str("[Unit]\n");
    s.push_str(&format!(
        "Description=cyberdeck: {} ({})\n",
        p.name,
        display_for(p)
    ));
    s.push_str("After=network.target\n\n");
    s.push_str("[Service]\n");
    s.push_str("Type=simple\n");
    s.push_str(&format!("ExecStart={exec}\n"));
    s.push_str("Restart=on-failure\n");
    s.push_str("RestartSec=5\n");
    for (k, v) in env_for(p) {
        s.push_str(&format!("Environment={k}={v}\n"));
    }
    if let Some(m) = p.mem_max_mb {
        s.push_str(&format!("MemoryMax={}M\n", m));
    }
    if let Some(m) = p.mem_swap_max_mb {
        s.push_str(&format!("MemorySwapMax={}M\n", m));
    }
    s.push_str("\n[Install]\n");
    s.push_str("WantedBy=default.target\n");
    s
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
