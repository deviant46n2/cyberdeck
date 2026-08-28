//! The comparison scorecard: how a raw trial becomes a number in 0..1.
//!
//! Kept apart from the report machinery so the science is auditable in one
//! place and testable in isolation. Nothing here touches the network or the
//! store — every function is a pure decision on the trial's raw ingredients.
//!
//! The scorecard is deliberately formulaic and fixed:
//!
//!   score = 0.6·quality + 0.4·throughput
//!
//! - `quality` (0..1) is lexical texture on the raw output: variety
//!   (unique/total whitespace tokens) blended with a bigram-repetition guard.
//!   Degenerate outputs ("yes yes yes…", empty, single words) score near 0.
//! - `throughput` (0..1) is the trial's tok/s normalized within its task group
//!   (max observed wins). It uses the recorded `tok_s` as-is — native engine
//!   timing when present, else wall-derived.
//!
//! A failed trial scores 0 outright; the report surface carries the engine
//! error string so a zero is accountable, not mysterious.

use std::collections::{HashMap, HashSet};

/// Fraction of whitespace-split tokens that appear exactly once.
fn lexical_variety(text: &str) -> f64 {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 4 {
        return 0.0;
    }
    let unique: HashSet<&str> = tokens.iter().copied().collect();
    unique.len() as f64 / tokens.len() as f64
}

/// Bigram-repetition ratio: fraction of adjacent-token pairs that repeat.
/// 1.0 = the same pair again and again ("yes yes yes yes"); 0.0 = no repeats.
fn bigram_repetition(text: &str) -> f64 {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 4 {
        return 1.0; // degenerate: nothing to distinguish
    }
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for w in tokens.windows(2) {
        *counts
            .entry((w[0].to_string(), w[1].to_string()))
            .or_insert(0) += 1;
    }
    let uniq = counts.values().filter(|&&c| c == 1).count();
    1.0 - uniq as f64 / counts.len() as f64
}

/// Lexical quality in 0..1: variety dampened by repetition (see the module
/// doc for the exact blend). Returns 0 for outputs too short to judge.
pub fn quality(text: &str) -> f64 {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 4 {
        return 0.0;
    }
    let variety = lexical_variety(text);
    let repetition = bigram_repetition(text);
    let texture = 1.0 - repetition;
    // Soft-floor instead of a pure product: a short distinct sentence should
    // not sink to 0 just because its few bigrams don't coexist.
    0.25 * variety + 0.75 * texture
}

/// Throughput normalized within a task group (0..1, max wins).
pub fn normalized_throughput(tok_s: Option<f64>, group_max: f64) -> f64 {
    match tok_s {
        Some(t) if t > 0.0 && group_max > 0.0 => (t / group_max).min(1.0),
        _ => 0.0,
    }
}

/// Independent-and-identical shuffle of indices by a fixed seed.
///
/// Used to assign opaque trial ids in a scrambled (but rerunnable) order. The
/// PRNG is an xorshift — plenty for presentation-blindness, not a crypto
/// boundary, which is fine: the ids are protocol scaffolding, not secrets.
pub fn shuffled_indices(n: usize, seed: u64) -> Vec<usize> {
    let mut xs: Vec<usize> = (0..n).collect();
    if n < 2 {
        return xs;
    }
    let mut s: u64 = seed.max(1);
    for i in (1..n).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let j = ((s % (i as u64 + 1)) as usize).min(i);
        xs.swap(i, j);
    }
    xs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degenerate_output_scores_zero() {
        assert_eq!(quality(""), 0.0);
        assert_eq!(quality("yes"), 0.0);
        assert_eq!(quality("yes yes"), 0.0);
        // A full token loop is punished hard even though a 0.25·variety floor
        // leaks in — the contract is relative ranking, not absolute zero.
        assert!(quality("yes yes yes yes yes yes") < 0.05);
    }

    #[test]
    fn varied_output_scores_higher_than_repetitive() {
        let varied =
            "The quick brown fox jumps over the lazy dog and keeps running south into the marsh.";
        let repetitive = "red red red red red red red red red red red red";
        let q_var = quality(varied);
        let q_rep = quality(repetitive);
        assert!(
            q_var > q_rep,
            "varied {q_var} should beat repetitive {q_rep}"
        );
    }

    #[test]
    fn normalized_throughput_scales() {
        assert_eq!(normalized_throughput(Some(3.0), 6.0), 0.5);
        assert_eq!(normalized_throughput(Some(6.0), 6.0), 1.0);
        assert_eq!(normalized_throughput(Some(7.0), 6.0), 1.0);
        assert_eq!(normalized_throughput(None, 6.0), 0.0);
    }

    #[test]
    fn seeds_permute_and_are_bijective() {
        let a = shuffled_indices(6, 1);
        let b = shuffled_indices(6, 2);
        assert_ne!(a, b);
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4, 5]);
    }
}
