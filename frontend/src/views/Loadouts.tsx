import { useState } from "react";
import * as api from "../api";

export default function Loadouts({
  profiles,
  onUnit,
}: {
  profiles: api.ProfileRow[];
  onUnit: (u: string) => void;
}) {
  const [sel, setSel] = useState<string>("");
  const [unit, setUnit] = useState<string>("");
  const [msg, setMsg] = useState<string>("");

  const selected = profiles.find((p) => p.name === sel);

  const preview = async (name: string) => {
    setSel(name);
    setMsg("");
    const r = await api.useProfile(name, true);
    setUnit(r.unit);
    onUnit(r.unit);
  };

  const apply = async (name: string) => {
    if (!confirm(`Apply '${name}' for real? Restarts the live service (takes .bak first).`))
      return;
    setMsg("applying…");
    const r = await api.useProfile(name, false);
    setUnit(r.unit);
    onUnit(r.unit);
    setMsg(`applied '${name}' — service restarted`);
  };

  if (profiles.length === 0) {
    return (
      <>
        <div className="view-title">LOADOUTS</div>
        <div className="stub">
          no loadouts yet — import a wrapper with{" "}
          <span className="mono">deck profile import</span>
        </div>
      </>
    );
  }

  return (
    <>
      <div className="view-title">LOADOUTS</div>
      <div className="grid cards">
        {profiles.map((p) => (
          <div
            key={p.name}
            className="card"
            style={
              sel === p.name
                ? { borderColor: "var(--magenta)" }
                : undefined
            }
          >
            <h3>{p.engine}</h3>
            <div className="big magenta" style={{ fontSize: 20 }}>{p.name}</div>
            <div className="dim" style={{ fontSize: 11, marginTop: 6 }}>
              {p.alias} @ :{p.port} · ctx {p.ctx.toLocaleString()}
            </div>
            <div className="row" style={{ marginTop: 12 }}>
              <button className="ghost" onClick={() => preview(p.name)}>
                PREVIEW
              </button>
              <button className="action" onClick={() => apply(p.name)}>
                APPLY
              </button>
            </div>
          </div>
        ))}
      </div>

      {selected && (
        <div style={{ marginTop: 20 }}>
          <div className="view-title">
            UNIT — {selected.name}
          </div>
          <pre className="unit">{unit || "(preview to render)"}</pre>
          {msg && <div className="dim" style={{ marginTop: 8 }}>{msg}</div>}
        </div>
      )}
    </>
  );
}
