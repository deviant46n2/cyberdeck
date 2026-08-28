//! Per-slot residency status: tells the PORT MAP whether each engine's unit is
//! active under systemd and whether it answers on its fixed live port. The DB
//! residency map (which profile is bound to which slot) is composed in by the
//! caller — this module only probes the live system, so it stays a pure engine
//! driver with no store dependency.

use std::process::Command;

use deck_core::profile::{Engine, EngineDescriptor};

use crate::health::health_ok_any;

/// Liveness verdict for one engine slot, independent of any stored binding.
#[derive(Debug, Clone)]
pub struct SlotProbe {
    /// True when the engine's systemd unit is loaded-and-active.
    pub unit_active: bool,
    /// True when the engine answers on its default live port.
    pub port_up: bool,
}

/// `systemctl --user is-active <unit>` returns the string "active" for a
/// running unit; anything else ("inactive", "failed", "activating", unknown)
/// means the unit is not serving. A missing user systemd falls back to "down".
pub fn unit_active(engine: Engine) -> bool {
    let unit = engine.systemd_unit();
    let out = Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "active",
        Err(_) => false,
    }
}

/// Combine the systemd and port-liveness signals for a single slot. Uses a
/// one-shot health_ok_any so a down port fails fast rather than hanging the
/// status command.
pub fn probe_slot(engine: Engine, host: &str) -> SlotProbe {
    let desc: &'static EngineDescriptor = engine.descriptor();
    SlotProbe {
        unit_active: unit_active(engine),
        port_up: health_ok_any(host, desc.default_port),
    }
}
