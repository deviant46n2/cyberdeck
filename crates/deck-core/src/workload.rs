//! Workload — first-class concept (ROADMAP Phase 2).
//! A workload defines what the user wants the model to be good at.
//! Tasks are `label=prompt` plus optional evaluator binding.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workload {
    pub id: String, // slug: coding, reasoning, instruction, assistant, agent, custom-*
    pub label: String,
    pub description: String,
    pub tasks: Vec<WorkloadTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkloadTask {
    pub label: String,
    pub prompt: String,
    /// evaluator id: exact|regex|json_schema|compile|lm_eval:humaneval|lexical-placeholder
    #[serde(default)]
    pub evaluator: String,
    /// evaluator config_json (regex pattern, json schema, harness tasks csv)
    #[serde(default)]
    pub evaluator_config: String,
}

pub fn seeded() -> Vec<Workload> {
    vec![
        Workload {
            id: "coding".into(),
            label: "Coding".into(),
            description: "code generation, debugging, refactoring, test generation".into(),
            tasks: vec![
                WorkloadTask { label: "humaneval".into(), prompt: "Write a Python function that returns the sum of two numbers.".into(), evaluator: "lm_eval:humaneval".into(), evaluator_config: "humaneval".into() },
                WorkloadTask { label: "mbpp".into(), prompt: "Write a function to check if a number is prime.".into(), evaluator: "lm_eval:mbpp".into(), evaluator_config: "mbpp".into() },
                WorkloadTask { label: "debug".into(), prompt: "Fix this bug: for i in range(len(arr)): print(arr[i+1])".into(), evaluator: "regex".into(), evaluator_config: "IndexError|bounds".into() },
            ],
        },
        Workload {
            id: "reasoning".into(),
            label: "Reasoning".into(),
            description: "mathematics, logic, multi-step reasoning".into(),
            tasks: vec![
                WorkloadTask { label: "gsm8k".into(), prompt: "Jan has 3 apples. She buys 2 more. How many does she have?".into(), evaluator: "lm_eval:gsm8k".into(), evaluator_config: "gsm8k".into() },
                WorkloadTask { label: "mmlu".into(), prompt: "Which element has atomic number 1? A) Helium B) Hydrogen C) Lithium".into(), evaluator: "lm_eval:mmlu".into(), evaluator_config: "mmlu".into() },
            ],
        },
        Workload {
            id: "instruction".into(),
            label: "Instruction Following".into(),
            description: "structured JSON, exact formatting, constraint following".into(),
            tasks: vec![
                WorkloadTask { label: "ifeval".into(), prompt: "Return JSON with keys name and age for a person named Alice aged 30.".into(), evaluator: "json_schema".into(), evaluator_config: r#"{"required":["name","age"]}"#.into() },
            ],
        },
        Workload {
            id: "assistant".into(),
            label: "General Assistant".into(),
            description: "Q&A, summarization, writing".into(),
            tasks: vec![
                WorkloadTask { label: "summarize".into(), prompt: "Summarize: The quick brown fox jumps over the lazy dog.".into(), evaluator: "lexical-placeholder".into(), evaluator_config: "".into() },
            ],
        },
        Workload {
            id: "agent".into(),
            label: "Agent".into(),
            description: "tool use, filesystem tasks, repo modification".into(),
            tasks: vec![
                WorkloadTask { label: "tool_use".into(), prompt: "List the files in the current directory and count them.".into(), evaluator: "regex".into(), evaluator_config: "files?|count".into() },
            ],
        },
    ]
}
