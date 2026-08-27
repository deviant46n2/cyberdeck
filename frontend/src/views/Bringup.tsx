import { useEffect, useState, useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import * as br from "../lib/br";
import * as api from "../api";

const PHASES = ["derive", "verify", "apply", "bench"];

const PHASE_LABEL: Record<string, string> = {
  idle: "",
  derive: "DERIVE — fit at max ctx",
  verify: "VERIFY — headless on test port",
  apply: "APPLY — install + start",
  bench: "BENCH — record tok/s",
  done: "DONE",
  error: "FAILED",
};

function gib(mb: number): string {
  return (mb / 1024).toFixed(2) + " GiB";
}

function fmtMB(mb: number): string {
  return mb.toLocaleString() + " MiB";
}

/**
 * Global bring-up status card (top-right). Appears when a LOAD run starts,
 * streams pipeline phases + log lines, and shows the final verdict with the
 * recorded tok/s. Dismissable once finished.
 *
 * On failure: shows the VRAM breakdown and tweak panel so the user can adjust
 * ctx, offload, or ngl and retry without restarting the full derive flow.
 */
export default function Bringup() {
  const state = useSyncExternalStore(br.subscribe, br.getSnapshot);

  useEffect(() => {
    br.init();
  }, []);

  // Listen for scan_started / scan_done events so the Downloads drawer can
  // show "scanning → indexed N models" feedback without requiring a separate
  // component to listen for Tauri events.
  useEffect(() => {
    let scanTimeout: ReturnType<typeof setTimeout> | null = null;

    import("@tauri-apps/api/event").then(({ listen }) => {
      listen("scan_started", () => {
        setScanMsg("scanning model index…");
        setScanVisible(true);
      });

      listen<{ indexed: number; dups: number }>("scan_done", (e) => {
        if (scanTimeout) clearTimeout(scanTimeout);
        const idx = e.payload.indexed;
        const dup = e.payload.dups;
        if (idx > 0) {
          setScanMsg(`✓ ${idx} model(s) indexed · ${dup} duplicate group(s) — jump to VAULT`);
        } else {
          setScanMsg("scan complete · no new models");
        }
        setScanVisible(true);
        scanTimeout = setTimeout(() => setScanVisible(false), 5000);
      });
    });

    return () => {
      if (scanTimeout) clearTimeout(scanTimeout);
    };
  }, []);

  const [scanMsg, setScanMsg] = useState("");
  const [scanVisible, setScanVisible] = useState(false);

  if (state.phase === "idle" && !scanVisible) return null;

  const phaseIdx = PHASES.indexOf(state.phase);
  const failed = state.result != null && !state.result.ok;
  const finished = state.result != null;
  const hasProfile = state.profile != null;

  return (
    <div className={`br-drawer ${failed ? "failed" : ""}`}>
      {/* Header */}
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 8 }}>
        <span className="mono" style={{ fontSize: 10, letterSpacing: 1, color: failed ? "var(--oom)" : state.running ? "var(--magenta)" : "var(--pass)" }}>
          {state.running ? `LOAD · ${state.phase.toUpperCase()}` : failed ? "LOAD FAILED" : "LOAD OK"}
        </span>
        {!state.running && (
          <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} onClick={br.dismiss}>
            ✕
          </button>
        )}
      </div>

      {/* Phase dots */}
      {state.running && (
        <div className="row" style={{ gap: 4, marginBottom: 10 }}>
          {PHASES.map((ph, i) => (
            <span
              key={ph}
              className="br-dot"
              title={PHASE_LABEL[ph]}
              style={{
                background:
                  i < phaseIdx ? "var(--pass)" : i === phaseIdx ? "var(--magenta)" : "#23232f",
                boxShadow: i === phaseIdx ? "0 0 8px rgba(255,46,196,0.5)" : undefined,
              }}
            />
          ))}
          <span className="mono" style={{ fontSize: 9, marginLeft: 6, color: "var(--dim2)" }}>
            {PHASE_LABEL[state.phase] ?? ""}
          </span>
        </div>
      )}

      {/* VRAM breakdown (always present after derive) */}
      {state.profile && state.result && state.result.fit && (
        <div className="card" style={{ background: "#07070e", padding: 8, marginBottom: 10, fontSize: 11 }}>
          <div className="mono" style={{ fontSize: 9, letterSpacing: 1, marginBottom: 6, color: "var(--dim2)" }}>
            VRAM BREAKDOWN
          </div>
          <div className="row" style={{ justifyContent: "space-between" }}>
            <span className="dim">Weights</span>
            <span className="mono">
              {gib(state.result.fit.weights_mb)}
              {state.result.fit.weights_ram_mb > 0 && (
                <span className="dim"> (GPU {gib(state.result.fit.weights_gpu_mb)} + RAM {gib(state.result.fit.weights_ram_mb)})</span>
              )}
            </span>
          </div>
          <div className="row" style={{ justifyContent: "space-between" }}>
            <span className="dim">KV @ ctx {state.result.ctx}</span>
            <span className="mono">{fmtMB(state.result.fit.kv_mb)}</span>
          </div>
          <div className="row" style={{ justifyContent: "space-between" }}>
            <span className="dim">Buffers</span>
            <span className="mono">{fmtMB(state.result.fit.buffers_mb)}</span>
          </div>
          <div className="row" style={{ justifyContent: "space-between", fontWeight: "bold", marginTop: 4 }}>
            <span>Total VRAM</span>
            <span className="mono">
              {gib(state.result.fit.model_vram_mb)} / {gib(state.result.fit.available_mb)}
            </span>
          </div>
          <div className="row" style={{ justifyContent: "space-between", marginTop: 2 }}>
            <span>Available for model</span>
            <span className="mono">{fmtMB(state.result.fit.available_for_model_mb)}</span>
          </div>
          <div className="row" style={{ justifyContent: "space-between", marginTop: 4 }}>
            <span>Verdict</span>
            <span className="mono" style={{ color: state.result.fit.verdict === "PASS" ? "var(--pass)" : state.result.fit.verdict === "WARN" ? "var(--warn)" : "var(--oom)" }}>
              {state.result.fit.verdict} · {fmtMB(state.result.fit.headroom_mb)} headroom
            </span>
          </div>
        </div>
      )}

      {/* Log lines */}
      {state.lines.length > 0 && (
        <div className="term" style={{ height: 96, marginTop: 0 }}>
          {state.lines.map((l, i) => (
            <div key={i}>{l}</div>
          ))}
        </div>
      )}

      {/* Failure result */}
      {finished && failed && state.result?.summary && (
        <div
          className="mono"
          style={{
            fontSize: 11,
            marginBottom: 10,
            color: "var(--oom)",
            lineHeight: 1.45,
          }}
        >
          {state.result.summary}
        </div>
      )}

      {/* Success result */}
      {finished && !failed && state.result?.summary && (
        <div
          className="mono"
          style={{
            fontSize: 11,
            marginBottom: 10,
            color: "var(--pass)",
            lineHeight: 1.45,
          }}
        >
          {state.result.summary}
        </div>
      )}

      {/* Tweak panel — only when we have a profile but the run failed */}
      {hasProfile && failed && state.profile && (
        <div className="card" style={{ background: "#07070e", padding: 8, marginTop: 8 }}>
          <div className="mono" style={{ fontSize: 9, letterSpacing: 1, marginBottom: 8, color: "var(--magenta)" }}>
            TWEAK &amp; RETRY
          </div>
          <TweakPanel profile={state.profile} onTweak={br.tweakWith} onApply={() => {}} />
        </div>
      )}

      {/* Scan toast */}
      {scanVisible && (
        <div className="card" style={{ background: "#07070e", padding: 8, marginTop: 8, cursor: "pointer" }}
             onClick={() => { setScanVisible(false); }}>
          <div className="mono" style={{ fontSize: 10, color: "var(--pass)" }}>
            {scanMsg}
          </div>
        </div>
      )}
    </div>
  );
}

/* ----------------------------------------------------------------- TweakPanel */

function TweakPanel({
  profile,
  onTweak,
  onApply,
}: {
  profile: api.Profile;
  onTweak: (profile: api.Profile, tweaks: { ctx?: number; kvBytes?: number; offload?: boolean; ngl?: number }) => void;
  onApply: (name: string) => void;
}) {
  const [ctx, setCtx] = useState(String(profile.ctx_size));
  const [offload, setOffload] = useState(profile.ft_backend === "offload");
  const [ngl, setNgl] = useState(String(profile.n_gpu_layers));

  const handleTweak = () => {
    onTweak(profile, {
      ctx: ctx ? parseInt(ctx, 10) : undefined,
      offload: offload,
      ngl: ngl ? parseInt(ngl, 10) : undefined,
    });
  };

  return (
    <div style={{ display: "grid", gap: 6 }}>
      <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
        <div style={{ flex: 1, minWidth: 120 }}>
          <div className="dim" style={{ fontSize: 9, marginBottom: 2 }}>CTX SIZE</div>
          <input
            type="number"
            value={ctx}
            onChange={(e) => setCtx(e.target.value)}
            style={{ width: "100%", fontSize: 11, background: "#0a0a12", color: "#e8e8f0", border: "1px solid #2a2a3a", padding: "3px 6px" }}
          />
        </div>
        <div style={{ width: 100 }}>
          <div className="dim" style={{ fontSize: 9, marginBottom: 2 }}>NGPU LAYERS</div>
          <input
            type="number"
            value={ngl}
            onChange={(e) => setNgl(e.target.value)}
            style={{ width: "100%", fontSize: 11, background: "#0a0a12", color: "#e8e8f0", border: "1px solid #2a2a3a", padding: "3px 6px" }}
          />
        </div>
        <div style={{ display: "flex", alignItems: "flex-end" }}>
          <label className="row" style={{ gap: 4, fontSize: 10 }}>
            <input
              type="checkbox"
              checked={offload}
              onChange={(e) => setOffload(e.target.checked)}
            />
            <span>OFFLOAD</span>
          </label>
        </div>
      </div>
      <div className="row" style={{ gap: 6 }}>
        <button className="ghost" onClick={handleTweak} style={{ fontSize: 9, padding: "3px 8px" }}>
          VERIFY TWEAKS
        </button>
        <button className="ghost" onClick={() => onApply(profile.name)} style={{ fontSize: 9, padding: "3px 8px" }}>
          SAVE AS LOADOUT
        </button>
      </div>
    </div>
  );
}
