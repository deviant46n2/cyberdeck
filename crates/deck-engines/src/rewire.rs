//! MANAGED client rewiring.
//!
//! When an engine swap is "managed", cyberdeck also repoints the client tools
//! (dsh, opencode) at the newly-active engine's port, so the rest of the user's
//! stack follows the swap without manual reconfiguration. Every file is backed
//! up to `<file>.bak.<nanos>` before it is touched, mirroring the systemd-unit
//! discipline. This is opt-in (the default `use` is Advisory and preserves the
//! alias+port contract); only `--managed` triggers rewiring.

use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RewireReport {
    pub client: String,
    pub path: String,
    pub status: String,
}

/// Replace the port in the first `http://127.0.0.1:<port>/…` URL occurring at or
/// after `block_anchor`, so the substitution is scoped to one provider block.
/// Returns the rewritten text, or `None` if the anchor/URL wasn't found.
fn set_port_in_block(
    text: &str,
    port: u16,
    block_anchor: &str,
    require_anchor: bool,
) -> Option<String> {
    let search_from = if require_anchor {
        text.find(block_anchor)?
    } else {
        0
    };
    let marker = "http://127.0.0.1:";
    let tail = &text[search_from..];
    let rel = tail.find(marker)?;
    let after = &tail[rel + marker.len()..];
    let port_len = after.find('/')?;
    let port_start = search_from + rel + marker.len();
    let port_end = port_start + port_len;
    if port_start == port_end {
        return None;
    }
    let mut s = text.to_string();
    s.replace_range(port_start..port_end, &port.to_string());
    Some(s)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn rewire_file(client: &str, path: &std::path::Path, port: u16, anchor: &str) -> RewireReport {
    if !path.exists() {
        return RewireReport {
            client: client.into(),
            path: path.display().to_string(),
            status: "not found — skipped".into(),
        };
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return RewireReport {
                client: client.into(),
                path: path.display().to_string(),
                status: format!("read error: {e}"),
            };
        }
    };
    let rewritten = match set_port_in_block(&text, port, anchor, true) {
        Some(r) => r,
        None => {
            return RewireReport {
                client: client.into(),
                path: path.display().to_string(),
                status: "no matching provider baseURL — skipped".into(),
            };
        }
    };
    if rewritten == text {
        return RewireReport {
            client: client.into(),
            path: path.display().to_string(),
            status: "already correct".into(),
        };
    }
    // Back up, then write.
    let _ = crate::backup_file(path);
    match std::fs::write(path, rewritten) {
        Ok(()) => RewireReport {
            client: client.into(),
            path: path.display().to_string(),
            status: format!("rewired → :{port}"),
        },
        Err(e) => RewireReport {
            client: client.into(),
            path: path.display().to_string(),
            status: format!("write error: {e}"),
        },
    }
}

/// Per-engine client-provider anchor for each config format. The port is
/// rewritten only inside the matching provider block, so with several
/// residents (llamacpp :18000 + freetoken :1919) one slot's rewire never
/// disturbs another's baseURL.
fn anchors_for(store_id: &str) -> Option<(&'static str, &'static str)> {
    // (dsh settings.yaml anchor, opencode.json anchor)
    match store_id {
        "llamacpp" => Some(("llamacpp:", "\"llamacpp\":")),
        "freetoken" => Some(("freetoken:", "\"freetoken\":")),
        "ollama" => Some(("ollama:", "\"ollama\":")),
        _ => None,
    }
}

/// Repoint the provider block for one engine slot in dsh + opencode. Always
/// returns a report per client so the UI can show exactly what changed.
pub fn rewire_clients_for(store_id: &str, port: u16) -> Vec<RewireReport> {
    let Some((dsh_anchor, oc_anchor)) = anchors_for(store_id) else {
        return vec![RewireReport {
            client: store_id.into(),
            path: "unknown engine — no client anchor".into(),
            status: "skipped".into(),
        }];
    };
    let dsh = home().join(".dsh/settings.yaml");
    let oc = home().join(".config/opencode/opencode.json");
    vec![
        rewire_file("dsh", &dsh, port, dsh_anchor),
        rewire_file("opencode", &oc, port, oc_anchor),
    ]
}

/// Repoint dsh + opencode at `port` for the llama.cpp alias slot (the default
/// / single-swap contract). Kept for back-compat; per-slot callers should use
/// `rewire_clients_for(store_id, port)`.
pub fn rewire_clients(port: u16) -> Vec<RewireReport> {
    rewire_clients_for("llamacpp", port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_scoped_port() {
        let text = r#"provider:
  llamacpp:
    baseURL: http://127.0.0.1:18000/v1
  freetoken:
    baseURL: http://127.0.0.1:1919/v1
"#;
        let out = set_port_in_block(text, 9999, "llamacpp:", true).unwrap();
        // llamacpp block changed, freetoken untouched
        assert!(out.contains("http://127.0.0.1:9999/v1"));
        assert!(out.contains("http://127.0.0.1:1919/v1"));
        assert_eq!(out.matches("9999").count(), 1);
    }

    #[test]
    fn no_anchor_means_no_change() {
        let text = "baseURL: http://127.0.0.1:18000/v1";
        assert!(set_port_in_block(text, 1919, "llamacpp:", true).is_none());
    }

    #[test]
    fn per_engine_anchors_exist() {
        assert_eq!(anchors_for("llamacpp"), Some(("llamacpp:", "\"llamacpp\":")));
        assert_eq!(anchors_for("freetoken"), Some(("freetoken:", "\"freetoken\":")));
        assert_eq!(anchors_for("ollama"), Some(("ollama:", "\"ollama\":")));
        assert_eq!(anchors_for("wat"), None);
    }

    #[test]
    fn rewrites_only_freetoken_block() {
        let text = "provider:\n  llamacpp:\n    baseURL: http://127.0.0.1:18000/v1\n  freetoken:\n    baseURL: http://127.0.0.1:1919/v1\n";
        let (ft_anchor, _) = anchors_for("freetoken").unwrap();
        let out = set_port_in_block(text, 9999, ft_anchor, true).unwrap();
        assert!(out.contains("http://127.0.0.1:9999/v1"), "freetoken block rewired");
        assert!(out.contains("http://127.0.0.1:18000/v1"), "llamacpp block untouched");
    }

    #[test]
    fn rewrites_ollama_json_block_only() {
        let text = "{\n  \"llamacpp\": { \"options\": { \"baseURL\": \"http://127.0.0.1:18000/v1\" } },\n  \"ollama\": { \"options\": { \"baseURL\": \"http://127.0.0.1:11434/v1\" } }\n}";
        let (_, oc_anchor) = anchors_for("ollama").unwrap();
        let out = set_port_in_block(text, 11440, oc_anchor, true).unwrap();
        assert!(out.contains("http://127.0.0.1:11440/v1"), "ollama block rewired");
        assert!(out.contains("http://127.0.0.1:18000/v1"), "llamacpp block untouched");
    }
}
