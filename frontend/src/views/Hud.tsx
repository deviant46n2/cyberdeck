import { useState } from "react";
import * as api from "../api";

function verdictClass(v: string): string {
  if (v === "PASS") return "pass";
  if (v === "WARN") return "warn";
  return "oom";
}

export default function Hud({
  models,
  dups,
  profiles,
  onUnit,
  onChanged,
}: {
  models: api.ModelRow[];
  dups: api.DupRow[];
  profiles: api.ProfileRow[];
  onUnit: (u: string) => void;
  onChanged: () => void;
}) {
  const [model, setModel] = useState("");
  const [ctx, setCtx] = useState(32768);
  const [fit, setFit] = useState<api.FitRow | null>(null);

  const wasted = dups.reduce((a, d) => a + d.wasted_gib, 0);
  const active = profiles[0];

  const runFit = async () => {
    if (!model) return;
    const f = await api.fit({
      model,
      ctx,
      kv_bytes: 0.5,
      ngl: 1.0,
      kv_layers: null,
      reserve: 1600,
    });
    setFit(f);
  };

  const applyActive = async () => {
    if (!active) return;
    if (!confirm(`Apply loadout '${active.name}'? This restarts the live service.`))
      return;
    const r = await api.useProfile(active.name, false);
    onUnit(r.unit);
    onChanged();
    alert(`applied '${active.name}'`);
  };

  return (
    <>
      <div className="view-title">HUD</div>
      <div className="grid cards">
        <div className="card">
          <h3>MODELS ON DISK</h3>
          <div className="big magenta">{models.length}</div>
        </div>
        <div className="card">
          <h3>WASTED (DUPES)</h3>
          <div className="big" style={{ color: wasted > 0 ? "var(--oom)" : "var(--pass)" }}>
            {wasted.toFixed(2)}<span style={{ fontSize: 14 }}> GiB</span>
          </div>
        </div>
        <div className="card">
          <h3>LOADOUTS</h3>
          <div className="big">{profiles.length}</div>
        </div>
        <div className="card">
          <h3>ACTIVE ENGINE</h3>
          <div className="big" style={{ fontSize: 18 }}>
            {active ? `${active.alias} @ :${active.port}` : "—"}
          </div>
        </div>
      </div>

      {active && (
        <div className="row" style={{ marginTop: 18 }}>
          <button className="action" onClick={applyActive}>
            APPLY {active.name.toUpperCase()}
          </button>
          <span className="dim">preserves alias+port · takes .bak first</span>
        </div>
      )}

      <div style={{ marginTop: 24 }}>
        <div className="view-title">FIT ESTIMATOR</div>
        <div className="card">
          <div className="row">
            <div style={{ flex: 1, minWidth: 260 }}>
              <label className="field">MODEL</label>
              <select value={model} onChange={(e) => setModel(e.target.value)}>
                <option value="">— select —</option>
                {models.map((m) => (
                  <option key={m.path} value={m.path}>
                    {m.name} ({m.footprint_gib.toFixed(1)} GiB)
                  </option>
                ))}
              </select>
            </div>
            <div style={{ flex: 1, minWidth: 260 }}>
              <label className="field">
                CONTEXT WINDOW — {ctx.toLocaleString()}
              </label>
              <input
                type="range"
                min={2048}
                max={131072}
                step={2048}
                value={ctx}
                onChange={(e) => setCtx(parseInt(e.target.value))}
              />
            </div>
            <div style={{ alignSelf: "flex-end" }}>
              <button className="action" onClick={runFit}>
                ESTIMATE
              </button>
            </div>
          </div>

          {fit && (
            <div style={{ marginTop: 16 }}>
              <div className="row" style={{ justifyContent: "space-between" }}>
                <span className="dim">VERDICT</span>
                <span className={`badge ${verdictClass(fit.verdict)}`}>
                  {fit.verdict}
                </span>
              </div>
              <table style={{ marginTop: 10 }}>
                <tbody>
                  <tr><td>weights</td><td className="mono">{fit.weights_mb} MiB</td></tr>
                  <tr><td>kv cache</td><td className="mono">{fit.kv_mb} MiB</td></tr>
                  <tr><td>buffers</td><td className="mono">{fit.buffers_mb} MiB</td></tr>
                  <tr><td>model VRAM</td><td className="mono">{fit.model_vram_mb} MiB</td></tr>
                  <tr><td>desktop reserve</td><td className="mono">{fit.overhead_mb} MiB</td></tr>
                  <tr>
                    <td>available-for-model</td>
                    <td className="mono">{fit.available_for_model_mb} MiB</td>
                  </tr>
                </tbody>
              </table>
            </div>
          )}
        </div>
      </div>
    </>
  );
}
