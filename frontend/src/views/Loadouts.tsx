import { useEffect, useState } from "react";
import * as api from "../api";
import LoadoutEditor, { defaultProfile } from "./LoadoutEditor";

export default function Loadouts({
  profiles,
  onUnit,
  onChanged,
}: {
  profiles: api.ProfileRow[];
  onUnit: (u: string) => void;
  onChanged: () => void;
}) {
  const [sel, setSel] = useState<string>("");
  const [unit, setUnit] = useState<string>("");
  const [msg, setMsg] = useState<string>("");
  const [editing, setEditing] = useState<api.Profile | null>(null);
  const [modelPaths, setModelPaths] = useState<string[]>([]);

  useEffect(() => {
    api
      .listModels()
      .then((m) => setModelPaths(m.map((x) => x.path)))
      .catch(() => {});
  }, []);

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

  const edit = async (name: string) => {
    try {
      const all = await api.listProfiles();
      const full = all.find((p) => p.name === name);
      if (full) setEditing(full as unknown as api.Profile);
    } catch (e) {
      setMsg(`open failed: ${String(e)}`);
    }
  };

  const remove = async (name: string) => {
    if (!confirm(`Delete loadout '${name}'?`)) return;
    try {
      await api.deleteProfile(name);
      onChanged();
      if (sel === name) {
        setSel("");
        setUnit("");
      }
    } catch (e) {
      setMsg(`delete failed: ${String(e)}`);
    }
  };

  if (editing) {
    return (
      <LoadoutEditor
        initial={editing}
        modelPaths={modelPaths}
        onClose={() => setEditing(null)}
        onSaved={() => {
          setEditing(null);
          onChanged();
        }}
      />
    );
  }

  return (
    <>
      <div className="view-title">LOADOUTS</div>
      <div className="row" style={{ marginBottom: 14 }}>
        <button
          className="action"
          onClick={() => setEditing(defaultProfile())}
        >
          + NEW LOADOUT
        </button>
        <span className="dim" style={{ fontSize: 11 }}>
          every flag editable · live fit · test load (OOM check)
        </span>
      </div>

      {profiles.length === 0 ? (
        <div className="stub">
          no loadouts yet — import a wrapper with{" "}
          <span className="mono">deck profile import</span>, or hit{" "}
          <span className="mono">+ NEW LOADOUT</span>
        </div>
      ) : (
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
              <div className="big magenta" style={{ fontSize: 20 }}>
                {p.name}
              </div>
              <div className="dim" style={{ fontSize: 11, marginTop: 6 }}>
                {p.alias} @ :{p.port} · ctx {p.ctx.toLocaleString()}
              </div>
              <div className="row" style={{ marginTop: 12, flexWrap: "wrap", gap: 8 }}>
                <button className="ghost" onClick={() => preview(p.name)}>
                  PREVIEW
                </button>
                <button className="action" onClick={() => apply(p.name)}>
                  APPLY
                </button>
                <button className="ghost" onClick={() => edit(p.name)}>
                  EDIT
                </button>
                <button className="ghost" onClick={() => remove(p.name)}>
                  DELETE
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {selected && (
        <div style={{ marginTop: 20 }}>
          <div className="view-title">UNIT — {selected.name}</div>
          <pre className="unit">{unit || "(preview to render)"}</pre>
          {msg && <div className="dim" style={{ marginTop: 8 }}>{msg}</div>}
        </div>
      )}
    </>
  );
}
