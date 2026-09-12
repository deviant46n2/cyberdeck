//! Transactional runtime/config hot-swap.
//!
//! Two invariants make swapping safe enough to trust:
//!   1. **verify-before-kill** — the candidate boots on its own test port
//!      while the live instance keeps serving. A candidate that fails never
//!      touches the live service.
//!   2. **rollback** — if the installed candidate fails its LIVE health check,
//!      the previous profile is reinstalled and restarted.
//!
//! The side effects are injected (`SwapOps`) so the transaction logic is
//! testable without systemd; production uses [`system_ops`].

use std::time::Duration;

use anyhow::Result;
use deck_core::profile::Profile;
use serde::Serialize;

use crate::health::{BringupOutcome, health_wait_any, verify_on_test_port};
use crate::systemd::{install, start, stop};
use crate::unit::unit_name_for;

/// Outcome of a swap attempt. `swapped == false` always means the live service
/// was left as it was (or restored) — never a half-applied state.
#[derive(Debug, Clone, Serialize)]
pub struct SwapReport {
    pub from_runtime: String,
    pub to_runtime: String,
    pub from_unit: String,
    pub to_unit: String,
    /// Candidate passed test-port verification.
    pub verified: bool,
    pub swapped: bool,
    pub rolled_back: bool,
    pub ctx: u32,
    pub tps: Option<f64>,
    pub summary: String,
}

/// Side-effect surface, injectable for tests. Each op mirrors a `systemd.rs`
/// primitive (or the test-port verifier) one-for-one.
pub struct SwapOps<'a> {
    pub verify: &'a dyn Fn(&Profile, u16, Duration) -> BringupOutcome,
    pub install: &'a dyn Fn(&Profile) -> Result<()>,
    pub start: &'a dyn Fn(&str) -> Result<()>,
    pub stop: &'a dyn Fn(&str) -> Result<()>,
    pub health: &'a dyn Fn(&str, u16, Duration) -> bool,
}

fn op_verify(p: &Profile, tp: u16, t: Duration) -> BringupOutcome {
    verify_on_test_port(p, tp, t)
}
fn op_install(p: &Profile) -> Result<()> {
    install(p, false).map(|_| ())
}
fn op_start(u: &str) -> Result<()> {
    start(u)
}
fn op_stop(u: &str) -> Result<()> {
    stop(u)
}
fn op_health(h: &str, p: u16, t: Duration) -> bool {
    health_wait_any(h, p, t)
}

/// The real systemd-backed operations.
pub fn system_ops() -> SwapOps<'static> {
    SwapOps {
        verify: &op_verify,
        install: &op_install,
        start: &op_start,
        stop: &op_stop,
        health: &op_health,
    }
}

/// Verify/live timings. Fixed in production; tests inject fakes that ignore
/// them, so they need not be parameters.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(180);
const LIVE_TIMEOUT: Duration = Duration::from_secs(60);

/// Swap `old` for `new`, verifying first unless `verify` is false. Returns a
/// report; only `swapped == true` means the live instance is the new config.
pub fn swap_with(
    ops: &SwapOps,
    old: &Profile,
    new: &Profile,
    new_test_port: u16,
    verify: bool,
    progress: &dyn Fn(&str),
) -> Result<SwapReport> {
    let old_unit = unit_name_for(old);
    let new_unit = unit_name_for(new);
    let mut report = SwapReport {
        from_runtime: old.runtime_key(),
        to_runtime: new.runtime_key(),
        from_unit: old_unit.clone(),
        to_unit: new_unit.clone(),
        verified: false,
        swapped: false,
        rolled_back: false,
        ctx: new.ctx_size,
        tps: None,
        summary: String::new(),
    };

    let mut candidate = new.clone();

    // 1. Verify before touching the live instance.
    if verify {
        progress(&format!(
            "verifying {} on :{} (live :{} untouched)",
            report.to_runtime, new_test_port, old.port
        ));
        let outcome = (ops.verify)(&candidate, new_test_port, VERIFY_TIMEOUT);
        report.tps = outcome.tok_per_sec;
        if outcome.verdict != "RUNNING" {
            report.summary = format!(
                "candidate rejected on the test port: {} ({}) — live instance untouched",
                outcome.summary, outcome.verdict
            );
            return Ok(report);
        }
        report.verified = true;
        candidate.ctx_size = outcome.ctx; // honor a ctx-ladder walk
        report.ctx = outcome.ctx;
    } else {
        report.verified = true; // explicitly waived
    }

    // 2. Commit: install candidate, stop the old slot if it differs, start.
    let commit = (ops.install)(&candidate)
        .and_then(|_| {
            if new_unit != old_unit {
                (ops.stop)(&old_unit)?;
            }
            (ops.start)(&new_unit)
        })
        .and_then(|_| {
            if (ops.health)(&candidate.host, candidate.port, LIVE_TIMEOUT) {
                Ok(())
            } else {
                Err(anyhow::anyhow!("live health check timed out"))
            }
        });

    match commit {
        Ok(()) => {
            report.swapped = true;
            report.summary =
                format!("swapped {} → {} at ctx {}", report.from_runtime, report.to_runtime, candidate.ctx_size);
        }
        Err(e) => {
            progress(&format!(
                "candidate failed ({e}); rolling back to {}",
                report.from_runtime
            ));
            let _ = (ops.install)(old);
            let _ = (ops.start)(&old_unit);
            if new_unit != old_unit {
                let _ = (ops.stop)(&new_unit);
            }
            report.rolled_back = true;
            report.summary = format!(
                "candidate {} failed live health ({e}); restored {}",
                report.to_runtime, report.from_runtime
            );
        }
    }
    Ok(report)
}

/// Production entry: real systemd ops + real timing.
pub fn swap(
    old: &Profile,
    new: &Profile,
    new_test_port: u16,
    verify: bool,
    progress: &dyn Fn(&str),
) -> Result<SwapReport> {
    swap_with(&system_ops(), old, new, new_test_port, verify, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn profile(runtime: &str, port: u16, unit_engine: deck_core::profile::Engine) -> Profile {
        Profile {
            engine: unit_engine,
            runtime_id: if runtime == "llamacpp" { None } else { Some(runtime.to_string()) },
            name: runtime.into(),
            model: "/m/x.gguf".into(),
            port,
            ctx_size: 32768,
            ..Profile::default()
        }
    }

    fn outcome(verdict: &str, ctx: u32) -> BringupOutcome {
        BringupOutcome {
            ctx,
            verdict: verdict.into(),
            summary: verdict.into(),
            tok_per_sec: Some(42.0),
        }
    }

    #[test]
    fn failed_verification_never_touches_the_live_slot() {
        let calls = RefCell::new(Vec::<String>::new());
        let ops = SwapOps {
            verify: &|_, _, _| outcome("OOM", 32768),
            install: &|_| {
                calls.borrow_mut().push("install".into());
                Ok(())
            },
            start: &|_| {
                calls.borrow_mut().push("start".into());
                Ok(())
            },
            stop: &|_| {
                calls.borrow_mut().push("stop".into());
                Ok(())
            },
            health: &|_, _, _| true,
        };
        let old = profile("llamacpp", 18000, deck_core::profile::Engine::LlamaCpp);
        let mut new = profile("freetoken", 1919, deck_core::profile::Engine::FreeToken);
        new.runtime_id = None;
        let r = swap_with(&ops, &old, &new, 18998, true, &|_| {}).unwrap();
        assert!(!r.verified && !r.swapped && !r.rolled_back);
        assert!(calls.borrow().is_empty(), "no install/start/stop on verify failure: {:?}", calls.borrow());
    }

    #[test]
    fn live_health_failure_rolls_back_to_the_old_profile() {
        let calls = RefCell::new(Vec::<String>::new());
        let ops = SwapOps {
            verify: &|_, _, _| outcome("RUNNING", 16384),
            install: &|p| {
                calls.borrow_mut().push(format!("install:{}", p.name));
                Ok(())
            },
            start: &|u| {
                calls.borrow_mut().push(format!("start:{u}"));
                Ok(())
            },
            stop: &|u| {
                calls.borrow_mut().push(format!("stop:{u}"));
                Ok(())
            },
            health: &|_, _, _| false, // candidate never comes up live
        };
        let old = profile("llamacpp", 18000, deck_core::profile::Engine::LlamaCpp);
        let mut new = profile("freetoken", 1919, deck_core::profile::Engine::FreeToken);
        new.runtime_id = None;
        let r = swap_with(&ops, &old, &new, 18998, true, &|_| {}).unwrap();
        assert!(r.verified && !r.swapped && r.rolled_back);
        // Candidate honored the ladder ctx.
        assert_eq!(r.ctx, 16384);
        let c = calls.borrow();
        assert!(c.iter().any(|x| x == "install:llamacpp"), "old reinstalled: {c:?}");
        assert!(c.iter().any(|x| x == "start:llama-server.service"), "old restarted: {c:?}");
    }

    #[test]
    fn successful_swap_installs_candidate_and_stops_the_old_slot() {
        let calls = RefCell::new(Vec::<String>::new());
        let ops = SwapOps {
            verify: &|_, _, _| outcome("RUNNING", 32768),
            install: &|p| {
                calls.borrow_mut().push(format!("install:{}", p.name));
                Ok(())
            },
            start: &|u| {
                calls.borrow_mut().push(format!("start:{u}"));
                Ok(())
            },
            stop: &|u| {
                calls.borrow_mut().push(format!("stop:{u}"));
                Ok(())
            },
            health: &|_, _, _| true,
        };
        let old = profile("llamacpp", 18000, deck_core::profile::Engine::LlamaCpp);
        let mut new = profile("freetoken", 1919, deck_core::profile::Engine::FreeToken);
        new.runtime_id = None;
        let r = swap_with(&ops, &old, &new, 18998, true, &|_| {}).unwrap();
        assert!(r.verified && r.swapped && !r.rolled_back);
        let c = calls.borrow();
        assert_eq!(c[0], "install:freetoken");
        assert!(c.iter().any(|x| x == "stop:llama-server.service"));
        assert!(c.iter().any(|x| x == "start:freetoken.service"));
    }
}
