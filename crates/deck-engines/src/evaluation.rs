//! Phase 2 Evaluator — pluggable, deterministic first, OSS-harness delegation.
//! `evaluate(output) -> Evaluation` where Evaluation {passed, score, details_json}.
//! Lexical placeholder stays for assistant tasks only.

use anyhow::Result;
use serde_json::json;

use deck_core::store::Evaluation;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub trait Evaluator: Send + Sync {
    fn id(&self) -> &str;
    fn evaluate(&self, output: &str, matrix_run_id: i64) -> Result<Evaluation>;
}

pub struct Exact { pub expected: String }
impl Evaluator for Exact {
    fn id(&self) -> &str { "exact" }
    fn evaluate(&self, output: &str, matrix_run_id: i64) -> Result<Evaluation> {
        let passed = output.trim() == self.expected.trim();
        Ok(Evaluation { id: 0, matrix_run_id, method: "exact".into(), passed, score: if passed { 1.0 } else { 0.0 }, details_json: json!({"expected": self.expected}).to_string(), at: now() })
    }
}

pub struct RegexEval { pub pattern: String }
impl Evaluator for RegexEval {
    fn id(&self) -> &str { "regex" }
    fn evaluate(&self, output: &str, matrix_run_id: i64) -> Result<Evaluation> {
        let re = regex::Regex::new(&self.pattern)?;
        let passed = re.is_match(output);
        Ok(Evaluation { id: 0, matrix_run_id, method: "regex".into(), passed, score: if passed { 1.0 } else { 0.0 }, details_json: json!({"pattern": self.pattern}).to_string(), at: now() })
    }
}

pub struct JsonSchema { pub schema: String }
impl Evaluator for JsonSchema {
    fn id(&self) -> &str { "json_schema" }
    fn evaluate(&self, output: &str, matrix_run_id: i64) -> Result<Evaluation> {
        let schema: serde_json::Value = serde_json::from_str(&self.schema).map_err(|e| anyhow::anyhow!("bad schema: {e}"))?;
        let val: serde_json::Value = match serde_json::from_str(output.trim()) {
            Ok(v) => v,
            Err(e) => return Ok(Evaluation { id: 0, matrix_run_id, method: "json_schema".into(), passed: false, score: 0.0, details_json: json!({"error": e.to_string()}).to_string(), at: now() }),
        };
        // minimal required-keys check (full jsonschema crate is heavy; add later)
        let passed = if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
            req.iter().all(|k| k.as_str().map(|s| val.get(s).is_some()).unwrap_or(false))
        } else { true };
        Ok(Evaluation { id: 0, matrix_run_id, method: "json_schema".into(), passed, score: if passed { 1.0 } else { 0.0 }, details_json: json!({"schema": schema, "valid": passed}).to_string(), at: now() })
    }
}

pub struct LexicalPlaceholder;
impl Evaluator for LexicalPlaceholder {
    fn id(&self) -> &str { "lexical-placeholder" }
    fn evaluate(&self, output: &str, matrix_run_id: i64) -> Result<Evaluation> {
        // preserve old scoring.rs behavior for assistant tasks until real eval
        let score = crate::scoring::quality(output);
        Ok(Evaluation { id: 0, matrix_run_id, method: "lexical-placeholder".into(), passed: score > 0.5, score, details_json: json!({"note":"placeholder, not benchmark truth"}).to_string(), at: now() })
    }
}

/// lm-eval harness delegation (EleutherAI). Gated on `which lm-eval`.
/// For now this evaluates via a cheap local proxy (regex on output for the
/// harness task name) and records harness_version; full harness invocation
/// (lm-eval --model hf --tasks X --output_path) is used in Phase 6 experiment
/// pipeline where a Python env is expected. This keeps the trait pluggable.
pub struct LmEval { pub tasks_csv: String }
impl Evaluator for LmEval {
    fn id(&self) -> &str { "lm_eval" }
    fn evaluate(&self, output: &str, matrix_run_id: i64) -> Result<Evaluation> {
        let harness_version = std::process::Command::new("lm-eval").arg("--version").output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_else(|| "not-installed".into());
        // placeholder: if harness is installed we would invoke it per-task;
        // for now score via lexical as proxy so pipeline doesn't block
        let proxy = crate::scoring::quality(output);
        Ok(Evaluation { id: 0, matrix_run_id, method: "lm_eval".into(), passed: proxy > 0.5, score: proxy, details_json: json!({"tasks": self.tasks_csv, "harness_version": harness_version.trim(), "note":"proxy until full harness wired in Phase 6"}).to_string(), at: now() })
    }
}

pub fn evaluator_for(task_evaluator: &str, config: &str) -> Box<dyn Evaluator> {
    if task_evaluator.starts_with("lm_eval") {
        return Box::new(LmEval { tasks_csv: if config.is_empty() { task_evaluator.trim_start_matches("lm_eval:").to_string() } else { config.to_string() } });
    }
    match task_evaluator {
        "exact" => Box::new(Exact { expected: config.to_string() }),
        "regex" => Box::new(RegexEval { pattern: config.to_string() }),
        "json_schema" => Box::new(JsonSchema { schema: config.to_string() }),
        _ => Box::new(LexicalPlaceholder),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_pass_fail() {
        let e = Exact { expected: "hello".into() };
        assert!(e.evaluate("hello", 1).unwrap().passed);
        assert!(!e.evaluate("hello ", 1).unwrap().passed == false); // trimmed so actually passes; just sanity
        assert!(!e.evaluate("hi", 1).unwrap().passed);
    }
    #[test]
    fn regex_match() {
        let e = RegexEval { pattern: "IndexError|bounds".into() };
        assert!(e.evaluate("fix IndexError here", 1).unwrap().passed);
        assert!(!e.evaluate("all good", 1).unwrap().passed);
    }
    #[test]
    fn json_schema_required() {
        let e = JsonSchema { schema: r#"{"required":["name","age"]}"#.into() };
        assert!(e.evaluate(r#"{"name":"Alice","age":30}"#, 1).unwrap().passed);
        assert!(!e.evaluate(r#"{"name":"Alice"}"#, 1).unwrap().passed);
        assert!(!e.evaluate("not json", 1).unwrap().passed);
    }
}
