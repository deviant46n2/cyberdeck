//! End-to-end report assembly: raw grid rows in, a blind scored report out.
//! The pure scorecard math lives with `scoring` (unit-tested there); this file
//! pins the pipeline — candidate grouping, opaque ids, verdict, failure surfacing.

use std::collections::HashSet;

use deck_core::store::MatrixRow;
use deck_engines::compare::report_from_rows;

fn row(model: &str, task: &str, run: u32, ok: bool, tok_s: Option<f64>, output: &str) -> MatrixRow {
    MatrixRow {
        engine: "llamacpp".into(),
        model: model.into(),
        ctx: 8192,
        task: task.into(),
        run,
        verdict: if ok { "RUNNING".into() } else { "ERROR".into() },
        summary: String::new(),
        gen_tokens: if ok { Some(10) } else { None },
        prompt_tokens: if ok { Some(20) } else { None },
        tok_s,
        tok_s_kind: "native".into(),
        wall_ms: 100,
        output: output.into(),
        at: 0,
    }
}

#[test]
fn report_blinds_order_and_picks_better_candidate() {
    // Good candidate: fast, varied output. Bad candidate: a token loop.
    let rows = vec![
        row(
            "good-Q4",
            "greet",
            0,
            true,
            Some(20.0),
            "A clear and varied response with structure and detail for the reader.",
        ),
        row(
            "good-Q4",
            "greet",
            1,
            true,
            Some(19.0),
            "A second clear response, equally varied and informative for the reader.",
        ),
        row(
            "loop-Q8",
            "greet",
            0,
            true,
            Some(40.0),
            "yes yes yes yes yes yes yes yes yes yes yes yes yes yes",
        ),
        row(
            "loop-Q8",
            "greet",
            1,
            true,
            Some(38.0),
            "no no no no no no no no no no no no no no no no no",
        ),
    ];
    let report = report_from_rows(rows, 7);
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(report.trials.len(), 4);
    let ids: HashSet<&str> = report.candidates.iter().map(|c| c.trial.as_str()).collect();
    assert_eq!(ids.len(), 2, "opaque ids must be unique");
    let winner = report
        .candidates
        .iter()
        .find(|c| c.verdict.as_deref() == Some("BEST"))
        .unwrap();
    assert_eq!(winner.model, "good-Q4", "quality must outrank raw speed");
    assert!(report.verdict.contains(&winner.trial));
}

#[test]
fn failed_trials_score_zero_and_surface_the_cause() {
    let rows = vec![
        row(
            "a",
            "greet",
            0,
            true,
            Some(10.0),
            "A distinctly worded and varied sentence for the purpose of this test.",
        ),
        MatrixRow {
            summary: "engine exited early with status exit status: 1".into(),
            ..row("b", "greet", 0, false, None, "")
        },
        MatrixRow {
            summary: "engine exited early with status exit status: 1".into(),
            ..row("b", "greet", 1, false, None, "")
        },
    ];
    let report = report_from_rows(rows, 1);
    let b = report.candidates.iter().find(|c| c.model == "b").unwrap();
    assert_eq!(b.ok_runs, 0);
    assert_eq!(b.mean_score, 0.0);
    assert_eq!(
        b.failure.as_deref(),
        Some("ERROR: engine exited early with status exit status: 1")
    );
    let zero = report.trials.iter().filter(|t| !t.ok).collect::<Vec<_>>();
    assert_eq!(zero.len(), 2);
    assert_eq!(zero[0].score, 0.0);
    assert_eq!(
        zero[0].error.as_deref(),
        Some("engine exited early with status exit status: 1")
    );
}

#[test]
fn same_inputs_same_seed_rerun_is_identical() {
    let rows = vec![
        row(
            "a",
            "greet",
            0,
            true,
            Some(10.0),
            "Distinct and varied prose for this test.",
        ),
        row(
            "b",
            "greet",
            0,
            true,
            Some(20.0),
            "Equally distinct and varied prose, longer.",
        ),
        row(
            "c",
            "greet",
            0,
            true,
            Some(30.0),
            "Third candidate with its own varied sentence.",
        ),
    ];
    let r1 = report_from_rows(rows.clone(), 99);
    let r2 = report_from_rows(rows, 99);
    let ids1: Vec<(&str, &str)> = r1
        .candidates
        .iter()
        .map(|c| (c.trial.as_str(), c.model.as_str()))
        .collect();
    let ids2: Vec<(&str, &str)> = r2
        .candidates
        .iter()
        .map(|c| (c.trial.as_str(), c.model.as_str()))
        .collect();
    assert_eq!(ids1, ids2);
}
