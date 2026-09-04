import { useEffect, useMemo, useState } from "react";
import * as api from "../api";
import * as br from "../lib/br";
import { useEngineList } from "../lib/engines";
import LoadoutEditor, { defaultProfile } from "./LoadoutEditor";

interface VaultProps {
  models: api.ModelRow[];
  dups: api.DupRow[];
  onRefresh: () => void;
  onReload: () => void;
}

function shortLabel(id: api.EngineId, display: string): string {
  if (id === "llamacpp") return "LCPP";
  if (id === "freetoken") return "FT";
  if (id === "ollama") return "OLLAMA";
  return display;
}

interface FlavorsCellProps {
  flavors: api.ProfileRow[];
  active: Set<string>;
  onApply: (name: string) => void;
  onAdd: () => void;
}

function FlavorsCell({ flavors, active, onApply, onAdd }: FlavorsCellProps) {
  return (
    <td>
      <div style={{ display: "flex", gap: 4, flexWrap: "wrap", maxWidth: 240 }}>
        {flavors.length === 0 ? (
          <span className="dim" style={{ fontSize: 9, padding: "3px 0" }}>none</span>
        ) : (
          flavors.map((f) => {
            const live = active.has(f.name);
            return (
              <button
                key={f.name}
                className={live ? "action" : "ghost"}
                style={{ fontSize: 9, padding: "2px 6px" }}
                title={`${live ? "● live — " : ""}${f.name} · ${f.engine} @ :${f.port} · ctx ${f.ctx.toLocaleString()} — click to apply`}
                onClick={() => onApply(f.name)}
              >
                {f.name}
              </button>
            );
          })
        )}
        <button
          className="ghost"
          style={{ fontSize: 9, padding: "2px 6px", borderColor: "var(--cyan)", color: "var(--cyan)" }}
          title="add a flavor — a named loadout for this model (different ctx/engine/offload)"
          onClick={onAdd}
        >
          + FLAVOR
        </button>
      </div>
    </td>
  );
}

export default function Vault({ models, dups, onRefresh, onReload }: VaultProps) {
  const [deleting, setDeleting] = useState<Set<string>>(new Set());
  const [flash, setFlash] = useState<string | null>(null);
  const [loadedPaths, setLoadedPaths] = useState<Set<string>>(new Set());
  const [loadedEngine, setLoadedEngine] = useState<Map<string, string>>(new Map());
  const [profiles, setProfiles] = useState<api.ProfileRow[]>([]);
  const [activeFlavors, setActiveFlavors] = useState<Set<string>>(new Set());
  const [reloadTick, setReloadTick] = useState(0);
  const [editing, setEditing] = useState<api.Profile | null>(null);
  const [ollamaRunning, setOllamaRunning] = useState<boolean>(false);
  const [ollamaBusy, setOllamaBusy] = useState(false);
  const dupIds = new Set(dups.flatMap((d) => d.members));
  const localEngines = useEngineList("LocalPath");

  // flavors = the loadouts bound to this model path (the model_id FK converges
  // them; the vault groups by path for display).
  const flavorMap = useMemo(() => {
    const byPath = new Map<string, api.ProfileRow[]>();
    for (const p of profiles) {
      const arr = byPath.get(p.model);
      if (arr) arr.push(p);
      else byPath.set(p.model, [p]);
    }
    return byPath;
  }, [profiles]);

  useEffect(() => {
    let alive = true;
    const poll = async () => {
      try {
        const [slots, pro, ollama] = await Promise.all([api.portMapStatus("127.0.0.1"), api.listProfiles(), api.ollamaIsRunning()]);
        const byName = new Map(pro.map((p) => [p.name, p]));
        const paths = new Set<string>();
        const engMap = new Map<string, string>();
        const active = new Set<string>();
        for (const s of slots) {
          if (s.state !== "down" && s.profile) {
            const pr = byName.get(s.profile);
            if (pr) { paths.add(pr.model); engMap.set(pr.model, s.engine); active.add(s.profile); }
          }
        }
        if (alive) { setLoadedPaths(paths); setLoadedEngine(engMap); setProfiles(pro); setActiveFlavors(active); setOllamaRunning(ollama); }
      } catch {}
    };
    void poll();
    const t = window.setInterval(() => void poll(), 10000);
    return () => { alive = false; window.clearInterval(t); };
  }, [reloadTick]);

  const load = (path: string, engine: api.EngineId) => {
    if (!confirm(`LOAD ${engine} — derive max-ctx, verify on test port, then go live?\n${path}`)) {
      return;
    }
    void br.startBringup(path, engine);
  };

  const test = (path: string, engine: api.EngineId) => {
    void br.startTest(path, engine);
  };

  const stop = async (path: string) => {
    const eng = loadedEngine.get(path);
    if (!eng) return;
    if (!confirm(`STOP ${eng} — frees VRAM, clears slot?`)) return;
    try { await api.engineStop(eng); setLoadedPaths((prev) => { const n = new Set(prev); n.delete(path); return n; }); } catch (e) { alert(String(e)); }
  };

  const toggleOllama = async () => {
    setOllamaBusy(true);
    try {
      if (ollamaRunning) {
        await api.ollamaStop();
        setOllamaRunning(false);
      } else {
        await api.ollamaStart();
        setOllamaRunning(true);
        // Trigger a rescan so ollama models appear
        setTimeout(() => { void onRefresh(); }, 1500);
      }
    } catch (e) {
      alert(`Ollama toggle failed: ${String(e)}`);
    } finally {
      setOllamaBusy(false);
    }
  };

  const remove = async (path: string) => {
    if (!confirm(`Delete "${path}"\n\nThis removes the index entry and deletes the file from disk.`)) return;
    setDeleting((prev) => new Set(prev).add(path));
    try {
      const deleted = await api.deleteModel(path, true);
      if (deleted.rows > 0) {
        setFlash(path);
        setTimeout(() => setFlash(null), 2000);
        void onReload();
      }
      if (!deleted.file_deleted) {
        alert(`Removed from the Vault, but the file is still on disk.\n${deleted.message}`);
      }
    } catch (e) {
      alert(`Delete failed: ${String(e)}`);
    } finally {
      setDeleting((prev) => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
    }
  };

  const applyFlavor = async (name: string) => {
    if (!confirm(`Apply flavor '${name}'?\n\nRestarts the live unit on its slot (takes .bak first).`)) return;
    try {
      await api.useProfile(name, false);
      setFlash(name);
      setTimeout(() => setFlash(null), 2000);
    } catch (e) {
      alert(`Apply failed: ${String(e)}`);
    }
  };

  const addFlavor = (m: api.ModelRow) => {
    const base = m.name || m.path.split("/").pop() || "model";
    setEditing({
      ...defaultProfile(),
      name: `${base}`,
      model: m.path,
      alias: base,
      ctx_ladder: [32768, 24576, 16384],
    });
  };

  if (editing) {
    return (
      <LoadoutEditor
        initial={editing}
        modelPaths={models.map((m) => m.path)}
        onClose={() => setEditing(null)}
        onSaved={() => {
          setEditing(null);
          setReloadTick((t) => t + 1);
          void onReload();
        }}
      />
    );
  }

  return (
    <>
      <div className="view-title" style={{ display: "flex", alignItems: "center", gap: 12 }}>
        VAULT
        <button
          className={ollamaRunning ? "action" : "ghost"}
          style={{ fontSize: 10, padding: "3px 10px", marginLeft: "auto" }}
          onClick={toggleOllama}
          disabled={ollamaBusy}
          title={ollamaRunning ? "Ollama is running — click to stop" : "Ollama is stopped — click to start"}
        >
          {ollamaBusy ? "..." : ollamaRunning ? "OLLAMA ● ON" : "OLLAMA ○ OFF"}
        </button>
      </div>

      {dups.length > 0 && (
        <div className="card" style={{ marginBottom: 16, borderColor: "var(--oom)" }}>
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
            <h3 style={{ color: "var(--oom)", margin: 0 }}>DUPLICATE SHARDS — WASTED SPACE</h3>
            <button className="ghost" style={{ fontSize: 9, padding: "3px 8px", borderColor: "var(--oom)", color: "var(--oom)" }} onClick={async () => { if (!confirm(`Delete ${dups.length} duplicate group(s)? Keeps cheapest per group, deletes rest from disk.`)) return; for (const d of dups) { try { await api.dedupDelete(d.identity, true); } catch (e) { alert(String(e)); } } void onRefresh(); }}>
              CLEAN DEDUP
            </button>
          </div>
          {dups.map((d) => (
            <div key={d.identity} style={{ margin: "8px 0" }}>
              <span className="badge oom">{d.wasted_gib.toFixed(2)} GiB</span>{" "}
              <span className="mono">{d.identity}</span>
              <button className="ghost" style={{ fontSize: 8, padding: "2px 6px", marginLeft: 8 }} onClick={async () => { if (!confirm(`Delete duplicates for ${d.identity}?`)) return; try { await api.dedupDelete(d.identity, true); void onRefresh(); } catch (e) { alert(String(e)); } }}>
                clean
              </button>
              <ul className="dim" style={{ margin: "4px 0 0 18px", fontSize: 11 }}>
                {d.members.map((m) => (
                  <li key={m}>{m}</li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}

      <div className="card">
        <table>
          <thead>
            <tr>
              <th>NAME</th>
              <th>QUANT</th>
              <th>ARCH</th>
              <th>TRAIN CTX</th>
              <th>SIZE</th>
              <th>PATH</th>
              <th>FLAVORS</th>
              <th>LOAD</th>
              <th>DELETE</th>
            </tr>
          </thead>
          <tbody>
            {models.length === 0 ? (
              <tr>
                <td colSpan={9} className="dim">
                  no models indexed — run a SCAN from HUD
                </td>
              </tr>
            ) : (
              models
                .filter((m) => flash !== m.path)
                .map((m) => {
                  const dup = dupIds.has(m.path);
                  const isDeleting = deleting.has(m.path);
                  const isLoaded = loadedPaths.has(m.path);
                  return (
                    <tr
                      key={m.path}
                      style={{
                        ...(dup ? { background: "rgba(248,81,73,0.06)" } : undefined),
                        ...(isLoaded ? { background: "rgba(63,185,80,0.1)", boxShadow: "inset 3px 0 0 var(--pass)" } : undefined),
                        opacity: isDeleting ? 0.3 : 1,
                        transition: "opacity 0.2s, background 0.2s",
                      }}
                    >
                    <td>{m.name}{isLoaded && <span className="badge" style={{ marginLeft: 6, background: "var(--pass)", color: "#000", fontSize: 8, padding: "2px 5px" }}>● LIVE</span>}</td>
                    <td>{m.quant ?? "—"}</td>
                    <td>{m.arch ?? "—"}</td>
                    <td className="mono">{m.ctx_train ? m.ctx_train.toLocaleString() : "—"}</td>
                    <td className="mono">{m.footprint_gib.toFixed(2)} GiB</td>
                    <td className="dim" style={{ maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {m.path}
                    </td>
                    <td>
                      <FlavorsCell
                        flavors={flavorMap.get(m.path) ?? []}
                        active={activeFlavors}
                        onApply={applyFlavor}
                        onAdd={() => addFlavor(m)}
                      />
                    </td>
                    <td>
                      <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
                        {localEngines.map((en) => (
                          <button
                            key={en.id}
                            className="ghost"
                            style={{ fontSize: 9, padding: "3px 7px" }}
                            title={`bring up via ${en.display} — derive max-ctx → verify on test port → live`}
                            onClick={() => load(m.path, en.id)}
                            disabled={m.path.toLowerCase().endsWith(".safetensors") && en.id !== "freetoken"}
                          >
                            {shortLabel(en.id, en.display)}
                          </button>
                        ))}
                      </div>
                      <div style={{ display: "flex", gap: 4, flexWrap: "wrap", marginTop: 4 }}>
                        <span className="dim" style={{ fontSize: 8, padding: "3px 0" }}>TEST</span>
                        {localEngines.map((en) => (
                          <button
                            key={en.id}
                            className="ghost"
                            style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--magenta)", color: "var(--magenta)" }}
                            title={`headless test via ${en.display} — derive + verify on test port, NOT applied`}
                            onClick={() => test(m.path, en.id)}
                            disabled={m.path.toLowerCase().endsWith(".safetensors") && en.id !== "freetoken"}
                          >
                            {shortLabel(en.id, en.display)}
                          </button>
                        ))}
                      </div>
                    </td>
                    <td>
                      <div style={{ display: "flex", gap: 4 }}>
                        {isLoaded && (
                          <button
                            className="ghost"
                            style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--oom)", color: "var(--oom)" }}
                            onClick={() => stop(m.path)}
                            title={`stop ${loadedEngine.get(m.path)} — frees VRAM (LM Studio-style)`}
                          >
                            STOP
                          </button>
                        )}
                        <button
                          className="ghost"
                          style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--oom)", color: "var(--oom)" }}
                          onClick={() => remove(m.path)}
                          disabled={isDeleting}
                          title={isDeleting ? "deleting..." : "delete from index and disk"}
                        >
                          {isDeleting ? "..." : "✕"}
                        </button>
                      </div>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}
