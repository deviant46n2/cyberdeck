import { useEffect, useState } from "react";
import * as api from "../api";

function verdictClass(v: string): string {
  if (v === "PASS") return "pass";
  if (v === "WARN") return "warn";
  return "oom";
}

const ENGINE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
];

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
  const [offload, setOffload] = useState(false);
  const [fit, setFit] = useState<api.FitRow | null>(null);
  const [status, setStatus] = useState<api.EngineStatus[]>([]);

  const wasted = dups.reduce((a, d) => a + d.wasted_gib, 0);
  const active = profiles[0];

  // Probe both engines' liveness on mount.
  useEffect(() => {
    Promise.all(
      ENGINE_NODES.map((n) =>
        api
          .engineStatus(n.engine, n.host, n.port)
          .catch(() => null)
      )
    ).then((res) => setStatus(res.filter(Boolean) as api.EngineStatus[]));
  }, []);

  const selectModel = (p: string) => {
    setModel(p);
    // Safetensors model-dirs (FreeToken) are offload-backed; GGUF files are not.
    setOffload(!p.toLowerCase().endsWith(".gguf"));
  };

  const runFit = async () => {
    if (!model) return;
    const f = await api.fit({
      model,
      ctx,
      kv_bytes: 0.5,
      ngl: 1.0,
      kv_layers: null,
      reserve: 1600,
      offload,
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
          <h3>ENGINE STATUS</h3>
          <div style={{ display: "flex", flexDirection: "column", gap: 6, marginTop: 4 }}>
            {ENGINE_NODES.map((n) => {
              const s = status.find((x) => x.engine === n.engine);
              return (
                <div key={n.engine} className="row" style={{ gap: 8 }}>
                  <span className={`dot ${s?.up ? "up" : "down"}`} />
                  <span className="mono" style={{ fontSize: 12 }}>
                    {n.engine === "LlamaCpp" ? "llamacpp" : "freetoken"} :{n.port}
                  </span>
                  <span className="dim" style={{ fontSize: 11 }}>
                    {s ? (s.up ? "ONLINE" : "offline") : "…"}
                  </span>
                </div>
              );
            })}
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
              <select value={model} onChange={(e) => selectModel(e.target.value)}>
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
            <div style={{ display: "flex", flexDirection: "column", gap: 8, minWidth: 160 }}>
              <label className="row" style={{ gap: 8, fontSize: 12 }}>
                <input
                  type="checkbox"
                  checked={offload}
                  onChange={(e) => setOffload(e.target.checked)}
                />
                FreeToken offload (RAM spill)
              </label>
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
                  <tr><td>weights (VRAM)</td><td className="mono">{fit.weights_mb} MiB</td></tr>
                  {fit.weights_ram_mb > 0 && (
                    <tr>
                      <td>weights (RAM spill)</td>
                      <td className="mono">{fit.weights_ram_mb} MiB</td>
                    </tr>
                  )}
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
