import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import * as br from "../lib/br";
import { useEngineList } from "../lib/engines";
import LoadoutEditor, { defaultProfile } from "./LoadoutEditor";
import PortMap from "./PortMap";

interface VaultProps {
  models: api.ModelRow[];
  dups: api.DupRow[];
  onRefresh: () => void;
  onReload: () => void;
}

function shortLabel(id: string): string {
  if (id === "llamacpp") return "LCPP";
  if (id === "uncensored") return "UNCENS";
  if (id === "freetoken") return "FT";
  if (id === "ollama") return "OLLAMA";
  return id;
}

// ---- Dropdown hook: portal-based menus that escape table overflow ----
// Strategy:
//   1. Menu renders via createPortal to document.body — no ancestor can clip it.
//   2. menuRef tracks the portal node — no className string selectors.
//   3. Menu uses onPointerDown + stopPropagation (React synthetic) to prevent
//      the native mousedown from reaching the document close handler.
//   4. Document-level mousedown (capture phase) closes open dropdowns when
//      clicking outside both the trigger button and the portal menu.
function useDropdown() {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const btnRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node | null;
      if (!t) return;
      // If click is on the trigger button, let toggle() handle it
      if (btnRef.current?.contains(t)) return;
      // If click is inside the portal menu, let the item handler handle it
      if (menuRef.current?.contains(t)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [open]);

  const toggle = () => {
    if (open) { setOpen(false); return; }
    if (btnRef.current) {
      const r = btnRef.current.getBoundingClientRect();
      setPos({ x: r.left, y: r.bottom + 2 });
    }
    setOpen(true);
  };

  return { open, setOpen, btnRef, menuRef, toggle, pos };
}

const DROP_STYLE: React.CSSProperties = {
  position: "fixed",
  background: "var(--bg2, #1a1a24)",
  border: "1px solid var(--dim2, #333)",
  borderRadius: 4,
  padding: "4px 0",
  minWidth: 160,
  zIndex: 99999,
  boxShadow: "0 4px 12px rgba(0,0,0,0.4)",
};

const DROP_ITEM = (active = false): React.CSSProperties => ({
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 8,
  padding: "4px 10px",
  fontSize: 10,
  cursor: "pointer",
  background: active ? "rgba(63,185,80,0.12)" : "transparent",
  color: active ? "var(--pass)" : "inherit",
  width: "100%",
  border: "none",
  borderRadius: 0,
  textAlign: "left",
  fontFamily: "inherit",
});

const DROP_DIVIDER: React.CSSProperties = {
  height: 1,
  background: "var(--dim2, #333)",
  margin: "3px 0",
};

/** Render a portal menu anchored to a trigger button's position. */
function DropMenu({
  open, menuRef, pos, onPointerDown, children,
}: {
  open: boolean;
  menuRef: React.RefObject<HTMLDivElement>;
  pos: { x: number; y: number };
  onPointerDown: (e: React.PointerEvent) => void;
  children: React.ReactNode;
}) {
  if (!open) return null;
  return createPortal(
    <div
      ref={menuRef as React.RefObject<HTMLDivElement>}
      style={{ ...DROP_STYLE, top: pos.y, left: pos.x }}
      onPointerDown={onPointerDown}
    >
      {children}
    </div>,
    document.body,
  );
}

// ---- Flavors dropdown ----
interface FlavorsCellProps {
  flavors: api.ProfileRow[];
  active: Set<string>;
  onApply: (name: string) => void;
  onStop: (name: string) => void;
  onAdd: () => void;
  onDuplicate: (name: string) => void;
}

function FlavorsCell({ flavors, active, onApply, onStop, onAdd, onDuplicate }: FlavorsCellProps) {
  const { open, setOpen, btnRef, menuRef, toggle, pos } = useDropdown();
  const liveCount = flavors.filter((f) => active.has(f.name)).length;

  // Stop native mousedown from reaching the document close handler
  const guard = (e: React.PointerEvent) => e.stopPropagation();

  return (
    <>
      <button
        ref={btnRef}
        className="ghost"
        style={{ fontSize: 9, padding: "3px 7px", display: "flex", alignItems: "center", gap: 4 }}
        onClick={toggle}
      >
        {flavors.length === 0 ? (
          <span className="dim">no flavors</span>
        ) : (
          <>
            <span>{flavors.length} flavor{flavors.length !== 1 ? "s" : ""}</span>
            {liveCount > 0 && <span style={{ color: "var(--pass)", fontSize: 8 }}>●{liveCount}</span>}
          </>
        )}
        <span className="dim" style={{ fontSize: 7 }}>▾</span>
      </button>
      <DropMenu open={open} menuRef={menuRef} pos={pos} onPointerDown={guard}>
        {flavors.length === 0 && (
          <div style={{ padding: "4px 10px", fontSize: 9, color: "var(--dim2)" }}>no saved loadouts</div>
        )}
        {flavors.map((f) => {
          const live = active.has(f.name);
          const overridden = f.origin === "override";
          const why = overridden
            ? `user override: ${f.overridden_fields.join(", ") || "edited"}`
            : "auto (fit-derived)";
          return (
            <div key={f.name} style={{ display: "flex", alignItems: "center" }}>
              <button
                style={{ ...DROP_ITEM(live), flex: 1 }}
                title={`${f.engine} @ :${f.port} · ctx ${f.ctx.toLocaleString()} · ${why}`}
                onClick={() => { setOpen(false); live ? onStop(f.name) : onApply(f.name); }}
              >
                <span>{overridden ? "✎ " : ""}{f.name}</span>
                <span className="dim" style={{ fontSize: 8, whiteSpace: "nowrap" }}>
                  {shortLabel(f.engine)} {f.ctx >= 1000 ? `${Math.round(f.ctx / 1000)}k` : f.ctx}
                </span>
              </button>
              <button
                style={{ ...DROP_ITEM(false), width: 22, justifyContent: "center", color: "var(--cyan)" }}
                title={`duplicate '${f.name}' as a new config`}
                onClick={() => { setOpen(false); onDuplicate(f.name); }}
              >
                ⧉
              </button>
            </div>
          );
        })}
        <div style={DROP_DIVIDER} />
        <button
          style={{ ...DROP_ITEM(false), color: "var(--cyan)" }}
          onClick={() => { setOpen(false); onAdd(); }}
        >
          + new flavor
        </button>
      </DropMenu>
    </>
  );
}

// ---- Load dropdown ----
interface LoadCellProps {
  engines: api.EngineDescriptor[];
  modelPath: string;
  flavorMap: Map<string, api.ProfileRow[]>;
  activeFlavors: Set<string>;
  onLoad: (path: string, engine: api.EngineId) => void;
  onTest: (path: string, engine: api.EngineId) => void;
  onStop: (path: string) => void;
  isLoaded: boolean;
}

function LoadCell({ engines, modelPath, flavorMap, activeFlavors, onLoad, onTest, onStop, isLoaded }: LoadCellProps) {
  const { open, setOpen, btnRef, menuRef, toggle, pos } = useDropdown();
  const flavors = flavorMap.get(modelPath) ?? [];
  const safetensors = modelPath.toLowerCase().endsWith(".safetensors");

  const guard = (e: React.PointerEvent) => e.stopPropagation();

  return (
    <>
      <button
        ref={btnRef}
        className="ghost"
        style={{
          fontSize: 9, padding: "3px 7px",
          ...(isLoaded ? { borderColor: "var(--pass)", color: "var(--pass)" } : undefined),
        }}
        onClick={toggle}
      >
        {isLoaded ? "● LIVE" : "LOAD ▾"}
      </button>
      <DropMenu open={open} menuRef={menuRef} pos={pos} onPointerDown={guard}>
        {engines.filter((e) => !safetensors || e.id === "freetoken").map((en) => {
          const saved = flavors.filter((f) => f.engine === en.id);
          const liveSaved = saved.find((f) => activeFlavors.has(f.name));
          return (
            <div key={en.id}>
              <div style={{ padding: "3px 10px 1px", fontSize: 8, color: "var(--dim2)", letterSpacing: 0.5 }}>
                {shortLabel(en.id)}
              </div>
              {saved.length > 0 ? (
                saved.map((f) => {
                  const live = activeFlavors.has(f.name);
                  return (
                    <button
                      key={f.name}
                      style={DROP_ITEM(live)}
                      onClick={() => { setOpen(false); onLoad(modelPath, en.id); }}
                      title={live ? "click to stop" : `apply: ${f.name} (ctx ${f.ctx.toLocaleString()})`}
                    >
                      <span>{f.name}</span>
                      <span className="dim" style={{ fontSize: 8 }}>
                        {f.ctx >= 1000 ? `${Math.round(f.ctx / 1000)}k` : f.ctx}
                      </span>
                    </button>
                  );
                })
              ) : (
                <button
                  style={DROP_ITEM(false)}
                  onClick={() => { setOpen(false); onLoad(modelPath, en.id); }}
                  title="derive fresh — fit math → verify → go live"
                >
                  <span className="dim">derive new</span>
                </button>
              )}
              {liveSaved && (
                <button
                  style={{ ...DROP_ITEM(false), color: "var(--oom)" }}
                  onClick={() => { setOpen(false); onStop(modelPath); }}
                >
                  stop {shortLabel(en.id)}
                </button>
              )}
            </div>
          );
        })}
        {isLoaded && (
          <>
            <div style={DROP_DIVIDER} />
            <button
              style={{ ...DROP_ITEM(false), color: "var(--oom)" }}
              onClick={() => { setOpen(false); onStop(modelPath); }}
            >
              stop all
            </button>
          </>
        )}
      </DropMenu>
    </>
  );
}

// ---- Test dropdown ----
interface TestCellProps {
  engines: api.EngineDescriptor[];
  modelPath: string;
  onTest: (path: string, engine: api.EngineId) => void;
}

function TestCell({ engines, modelPath, onTest }: TestCellProps) {
  const { open, setOpen, btnRef, menuRef, toggle, pos } = useDropdown();
  const safetensors = modelPath.toLowerCase().endsWith(".safetensors");

  const guard = (e: React.PointerEvent) => e.stopPropagation();

  return (
    <>
      <button
        ref={btnRef}
        className="ghost"
        style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--magenta)", color: "var(--magenta)" }}
        onClick={toggle}
      >
        TEST ▾
      </button>
      <DropMenu open={open} menuRef={menuRef} pos={pos} onPointerDown={guard}>
        {engines.filter((e) => !safetensors || e.id === "freetoken").map((en) => (
          <button
            key={en.id}
            style={DROP_ITEM()}
            onClick={() => { setOpen(false); onTest(modelPath, en.id); }}
            title={`headless test via ${en.display} — derive + verify on test port, NOT applied`}
          >
            {shortLabel(en.id)}
          </button>
        ))}
      </DropMenu>
    </>
  );
}

// ---- main Vault ----
export default function Vault({ models, dups, onRefresh, onReload }: VaultProps) {
  const [deleting, setDeleting] = useState<Set<string>>(new Set());
  const [flash, setFlash] = useState<string | null>(null);
  const [applying, setApplying] = useState<string | null>(null); // status toast: shows "Applying profile…", errors, etc.
  const [loadedPaths, setLoadedPaths] = useState<Set<string>>(new Set());
  const [loadedEngine, setLoadedEngine] = useState<Map<string, string>>(new Map());
  const [profiles, setProfiles] = useState<api.ProfileRow[]>([]);
  const [activeFlavors, setActiveFlavors] = useState<Set<string>>(new Set());
  const [reloadTick, setReloadTick] = useState(0);
  const [editing, setEditing] = useState<api.Profile | null>(null);
  const [ollamaRunning, setOllamaRunning] = useState<boolean>(false);
  const [syncErr, setSyncErr] = useState<string | null>(null);
  const [syncAt, setSyncAt] = useState<number>(0);
  const [ollamaBusy, setOllamaBusy] = useState(false);
  const [extServices, setExtServices] = useState<api.ExternalService[]>([]);
  const [extBusy, setExtBusy] = useState<string | null>(null);
  const dupIds = new Set(dups.flatMap((d) => d.members));
  const localEngines = useEngineList("LocalPath");

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
      // allSettled: one failing door must not blank the rest, and failures
      // surface in the sync line instead of a silent catch — an empty vault
      // with no error is otherwise indistinguishable from "nothing loaded".
      const [slotsR, proR, ollamaR, extR] = await Promise.allSettled([
        api.portMapStatus("127.0.0.1"),
        api.listProfiles(),
        api.ollamaIsRunning(),
        api.externalServices(),
      ]);
      const errs: string[] = [];
      const slots = slotsR.status === "fulfilled" ? slotsR.value : [];
      const pro = proR.status === "fulfilled" ? proR.value : null;
      if (slotsR.status === "rejected") errs.push(`portmap: ${String(slotsR.reason)}`);
      if (proR.status === "rejected") errs.push(`profiles: ${String(proR.reason)}`);
      if (ollamaR.status === "rejected") errs.push(`ollama: ${String(ollamaR.reason)}`);
      if (extR.status === "rejected") errs.push(`external: ${String(extR.reason)}`);
      if (!alive) return;
      if (pro) {
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
        setLoadedPaths(paths); setLoadedEngine(engMap); setProfiles(pro); setActiveFlavors(active);
      }
      if (ollamaR.status === "fulfilled") setOllamaRunning(ollamaR.value);
      if (extR.status === "fulfilled") setExtServices(extR.value);
      setSyncErr(errs.length ? errs.join(" · ") : null);
      setSyncAt(Date.now());
    };
    void poll();
    const t = window.setInterval(() => void poll(), 10000);
    return () => { alive = false; window.clearInterval(t); };
  }, [reloadTick]);

  // Live progress from the backend apply path (unit install → start →
  // health check → ctx-ladder retries). Keeps the toast truthful during the
  // multi-minute ladder walk instead of a static "Applying…".
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void listen<{ name: string; step: string }>("use-profile-step", (e) => {
      setApplying(`${e.payload.name}: ${e.payload.step}`);
    }).then((u) => { unlisten = u; });
    return () => { unlisten?.(); };
  }, []);

  const load = async (path: string, engine: api.EngineId) => {
    const existing = (flavorMap.get(path) ?? []).filter((f) => f.engine === engine);
    if (existing.length > 0) {
      const chosen = existing.find((f) => activeFlavors.has(f.name)) ?? existing[0];
      setApplying(`Applying ${chosen.name}…`);
      console.log(`[vault] useProfile start: ${chosen.name}`);
      const t0 = Date.now();
      try {
        await api.useProfile(chosen.name, false);
        console.log(`[vault] useProfile done in ${Date.now() - t0}ms`);
        setFlash(chosen.name);
        setApplying(`Applied ${chosen.name}`);
        setTimeout(() => { setFlash(null); setApplying(null); }, 2000);
      } catch (e) {
        console.error(`[vault] useProfile failed:`, e);
        setApplying(`ERROR: ${String(e)}`);
        setTimeout(() => setApplying(null), 5000);
      }
      return;
    }
    if (!confirm(`DERIVE ${engine} — fit math → verify on test port → go live?\n${path}`)) return;
    void br.startBringup(path, engine);
  };

  const test = (path: string, engine: api.EngineId) => {
    void br.startTest(path, engine);
  };

  const stop = async (path: string) => {
    const eng = loadedEngine.get(path);
    if (!eng) return;
    try {
      await api.engineStop(eng);
      setLoadedPaths((prev) => { const n = new Set(prev); n.delete(path); return n; });
      setReloadTick((t) => t + 1);
    } catch (e) {
      alert(String(e));
    }
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
        setTimeout(() => { void onRefresh(); }, 1500);
      }
    } catch (e) {
      alert(`Ollama toggle failed: ${String(e)}`);
    } finally {
      setOllamaBusy(false);
    }
  };

  const forget = async (path: string) => {
    if (!confirm(`Forget "${path}"?\n\nRemoves the library record. FILES STAY on disk and will resurface as orphaned.`)) return;
    setDeleting((prev) => new Set(prev).add(path));
    try {
      await api.forgetModel(path);
      setFlash(path);
      setTimeout(() => setFlash(null), 2000);
      void onReload();
    } catch (e) {
      alert(`Forget failed: ${String(e)}`);
    } finally {
      setDeleting((prev) => { const n = new Set(prev); n.delete(path); return n; });
    }
  };

  const removeFiles = async (path: string, sizeGib: number) => {
    if (!confirm(`Remove local file?\n\n${path}\n${sizeGib.toFixed(2)} GiB will be freed.\n\nThe library record is dropped too.`)) return;
    setDeleting((prev) => new Set(prev).add(path));
    try {
      const r = await api.removeModelFiles([path]);
      if (r.missing.length) alert(`Not on disk (record kept):\n${r.missing.join("\n")}`);
      setFlash(path);
      setTimeout(() => setFlash(null), 2000);
      void onReload();
    } catch (e) {
      alert(`Remove failed: ${String(e)}`);
    } finally {
      setDeleting((prev) => { const n = new Set(prev); n.delete(path); return n; });
    }
  };

  const makeItWork = async (path: string) => {
    try {
      const cands = await api.fitCandidates(path);
      if (!cands.length) {
        alert(`No viable runtime/configuration for this model on this machine.`);
        return;
      }
      const best = cands[0];
      if (!confirm(`⚡ Make it work?\n\n${best.display} [${best.status}]\nctx ${best.max_ctx.toLocaleString()} · ${best.kv_label} KV\n\n${best.why}\n\nDerive → verify on test port → go live?`)) return;
      void br.startBringup(path, best.runtime_id as api.EngineId);
    } catch (e) {
      alert(`Fit check failed: ${String(e)}`);
    }
  };

  const applyFlavor = async (name: string) => {
    setApplying(`Applying ${name}…`);
    console.log(`[vault] applyFlavor start: ${name}`);
    const t0 = Date.now();
    try {
      await api.useProfile(name, false);
      console.log(`[vault] applyFlavor done in ${Date.now() - t0}ms`);
      setFlash(name);
      setApplying(`Applied ${name}`);
      setTimeout(() => { setFlash(null); setApplying(null); }, 2000);
    } catch (e) {
      console.error(`[vault] applyFlavor failed:`, e);
      setApplying(`ERROR: ${String(e)}`);
      setTimeout(() => setApplying(null), 5000);
    }
  };

  const stopFlavor = async (name: string) => {
    const f = profiles.find((p) => p.name === name);
    if (!f) return;
    try {
      await api.engineStop(f.engine);
      setReloadTick((t) => t + 1);
    } catch (e) {
      alert(`Stop failed: ${String(e)}`);
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

  // Clone a known-good config so the source stays intact while the copy is
  // tuned — the "Duplicate → change ctx to 64K" workflow.
  const duplicateFlavor = async (name: string) => {
    const newName = prompt(`Duplicate '${name}' as:`, `${name}-copy`);
    if (!newName || newName === name) return;
    try {
      await api.duplicateProfile(name, newName);
      setReloadTick((t) => t + 1);
    } catch (e) {
      alert(`Duplicate failed: ${String(e)}`);
    }
  };

  const toggleExtService = async (unit: string, active: boolean) => {
    setExtBusy(unit);
    try {
      if (active) {
        await api.externalServiceStop(unit);
      } else {
        await api.externalServiceStart(unit);
      }
      setReloadTick((t) => t + 1);
    } catch (e) {
      alert(`Service toggle failed: ${String(e)}`);
    } finally {
      setExtBusy(null);
    }
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

      <PortMap onChanged={() => setReloadTick((t) => t + 1)} />

      {extServices.length > 0 && (
        <div className="card" style={{ marginBottom: 16 }}>
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
            <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--cyan)", margin: 0 }}>EXTERNAL SERVICES</h3>
            <span className="dim" style={{ fontSize: 9 }}>
              hand-rolled systemd units · start/stop independently
            </span>
          </div>
          {extServices.map((s) => (
            <div key={s.unit} className="row" style={{ gap: 8, marginTop: 8, alignItems: "center" }}>
              <span
                title={s.active ? "running" : "stopped"}
                style={{
                  width: 7, height: 7, borderRadius: "50%", flexShrink: 0,
                  background: s.active ? "var(--pass)" : "var(--dim2)",
                  boxShadow: "none",
                }}
              />
              <span className="mono" style={{ width: 140, fontSize: 11 }}>{s.display}</span>
              <span className="mono dim" style={{ width: 58, fontSize: 10 }}>
                {s.port != null ? `:${s.port}` : ""}
              </span>
              <span className="mono" style={{ flex: 1, fontSize: 10, color: "var(--dim2)" }}>
                {s.unit}
              </span>
              <span className="mono" style={{ fontSize: 9, width: 92, textAlign: "right", color: s.active ? "var(--pass)" : "var(--dim2)" }}>
                {s.active ? "● active" : "○ dead"}
              </span>
              <button
                className="ghost"
                style={{
                  fontSize: 9, padding: "3px 7px",
                  borderColor: s.active ? "var(--oom)" : "var(--pass)",
                  color: s.active ? "var(--oom)" : "var(--pass)",
                }}
                onClick={() => toggleExtService(s.unit, s.active)}
                disabled={extBusy === s.unit}
                title={s.active ? `stop ${s.unit}` : `start ${s.unit}`}
              >
                {extBusy === s.unit ? "…" : s.active ? "STOP" : "START"}
              </button>
            </div>
          ))}
        </div>
      )}

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

      <div className="dim" style={{ fontSize: 10, marginBottom: 6 }}>
        {syncErr ? (
          <span style={{ color: "var(--oom)" }}>vault sync failed: {syncErr}</span>
        ) : (
          <span>vault synced{syncAt ? ` ${new Date(syncAt).toLocaleTimeString()}` : ""} · {loadedPaths.size} live model{loadedPaths.size === 1 ? "" : "s"}</span>
        )}
      </div>

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
              <th>ACTIONS</th>
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
                        onStop={stopFlavor}
                        onAdd={() => addFlavor(m)}
                        onDuplicate={duplicateFlavor}
                      />
                    </td>
                    <td>
                      <LoadCell
                        engines={localEngines}
                        modelPath={m.path}
                        flavorMap={flavorMap}
                        activeFlavors={activeFlavors}
                        onLoad={load}
                        onTest={test}
                        onStop={stop}
                        isLoaded={isLoaded}
                      />
                    </td>
                    <td>
                      <div style={{ display: "flex", gap: 4 }}>
                        <button
                          className="ghost"
                          style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--pass)", color: "var(--pass)" }}
                          onClick={() => makeItWork(m.path)}
                          title="fit check across runtimes → derive → verify → go live"
                        >
                          ⚡
                        </button>
                        <TestCell engines={localEngines} modelPath={m.path} onTest={test} />
                        <button
                          className="ghost"
                          style={{ fontSize: 9, padding: "3px 7px" }}
                          onClick={() => forget(m.path)}
                          disabled={isDeleting}
                          title="forget: drop the library record, files stay on disk"
                        >
                          {isDeleting ? "..." : "forget"}
                        </button>
                        <button
                          className="ghost"
                          style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--oom)", color: "var(--oom)" }}
                          onClick={() => removeFiles(m.path, m.footprint_gib)}
                          disabled={isDeleting}
                          title={`remove local file (${m.footprint_gib.toFixed(2)} GiB freed)`}
                        >
                          ✕ file
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

      {applying && (
        <div
          style={{
            position: "fixed",
            bottom: 16,
            left: "50%",
            transform: "translateX(-50%)",
            background: applying.startsWith("ERROR") ? "var(--oom, #f44)" : "var(--accent, #5b8bf5)",
            color: "#fff",
            padding: "8px 18px",
            borderRadius: 6,
            fontSize: 12,
            fontWeight: 600,
            zIndex: 99999,
            boxShadow: "0 4px 16px rgba(0,0,0,0.4)",
            whiteSpace: "nowrap",
          }}
        >
          {applying}
        </div>
      )}
    </>
  );
}
