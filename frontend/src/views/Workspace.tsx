import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../api";
import TuiWindow from "../components/TuiWindow";

const LOCAL_PREFIXES = ["llamacpp/", "freetoken/", "ollama/"];

export default function Workspace({
  models,
  profiles,
  onChanged,
}: {
  models: api.ModelRow[];
  dups: api.DupRow[];
  profiles: api.ProfileRow[];
  onChanged: () => void;
}) {
  const [panes, setPanes] = useState<{ id: string; dir: string; pos: { x: number; y: number } }[]>([]);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const zoomRef = useRef(1);
  const panRef = useRef({ x: 0, y: 0 });
  const spaceHeld = useRef(false);
  useEffect(() => { zoomRef.current = zoom; }, [zoom]);
  useEffect(() => { panRef.current = pan; }, [pan]);
  const canvasRef = useRef<HTMLDivElement>(null);

  const [selectedTui, setSelectedTui] = useState<string | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; nodeId: string } | null>(null);
  const [selectMode, setSelectMode] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [tuiRoles, setTuiRoles] = useState<Map<string, string>>(new Map());
  const [tuiEdges, setTuiEdges] = useState<api.WorkflowEdge[]>([]);
  const [connectingFrom, setConnectingFrom] = useState<string | null>(null);
  const [spawnOpen, setSpawnOpen] = useState(false);
  const [spawnDir, setSpawnDir] = useState("/home/deviant/Projects/cyberdeck");
  const [spawnRole, setSpawnRole] = useState("");
  const [tuiErr, setTuiErr] = useState("");

  // Canvas text labels
  type CanvasLabel = { id: string; pos: { x: number; y: number }; text: string };
  const [labels, setLabels] = useState<CanvasLabel[]>([]);
  const [editingLabel, setEditingLabel] = useState<string | null>(null);

  // Active tool: "select" | "text" | "delete"
  const [tool, setTool] = useState<"select" | "text" | "delete">("select");

  // Saved workspaces
  type SavedWorkspace = {
    id: string;
    name: string;
    panes: { id: string; dir: string; pos: { x: number; y: number } }[];
    roles: [string, string][];
    edges: api.WorkflowEdge[];
    labels: { id: string; pos: { x: number; y: number }; text: string }[];
    zoom: number;
    pan: { x: number; y: number };
    savedAt: number;
  };
  const [workspaces, setWorkspaces] = useState<SavedWorkspace[]>([]);
  const [wsPanelOpen, setWsPanelOpen] = useState(false);
  const [wsName, setWsName] = useState("");
  const LS_KEY = "cyberdeck-workspaces";

  // Load saved workspaces on mount
  useEffect(() => {
    try {
      const raw = localStorage.getItem(LS_KEY);
      if (raw) setWorkspaces(JSON.parse(raw));
    } catch { /* ignore */ }
  }, []);

  const persistWorkspaces = (ws: SavedWorkspace[]) => {
    setWorkspaces(ws);
    localStorage.setItem(LS_KEY, JSON.stringify(ws));
  };

  const saveWorkspace = () => {
    const name = wsName.trim() || `workspace ${new Date().toLocaleString()}`;
    const ws: SavedWorkspace = {
      id: `ws-${Date.now().toString(36)}`,
      name,
      panes: panes.map((p) => ({ ...p })),
      roles: [...tuiRoles.entries()],
      edges: [...tuiEdges],
      labels: labels.map((l) => ({ ...l })),
      zoom,
      pan: { ...pan },
      savedAt: Date.now(),
    };
    persistWorkspaces([ws, ...workspaces]);
    setWsName("");
  };

  const loadWorkspace = (ws: SavedWorkspace) => {
    setPanes(ws.panes);
    setTuiRoles(new Map(ws.roles));
    setTuiEdges(ws.edges);
    setLabels(ws.labels);
    setZoom(ws.zoom);
    setPan(ws.pan);
    setSelectedTui(null);
    setSelectMode(false);
    setSelected(new Set());
    setConnectingFrom(null);
    setWsPanelOpen(false);
  };

  const deleteWorkspace = (id: string) => {
    persistWorkspaces(workspaces.filter((w) => w.id !== id));
  };



  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") { setConnectingFrom(null); setCtxMenu(null); setTool("select"); setEditingLabel(null); }
      if (e.key === " ") { e.preventDefault(); spaceHeld.current = true; }
    };
    const u = (e: KeyboardEvent) => { if (e.key === " ") spaceHeld.current = false; };
    window.addEventListener("keydown", h);
    window.addEventListener("keyup", u);
    return () => { window.removeEventListener("keydown", h); window.removeEventListener("keyup", u); };
  }, []);

  useEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (spaceHeld.current || e.ctrlKey || e.metaKey) {
        e.preventDefault();
        setPan((p) => ({ x: p.x - (e.deltaX || 0), y: p.y - e.deltaY }));
        return;
      }
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const mx = e.clientX - rect.left;
      const my = e.clientY - rect.top;
      const oldZ = zoomRef.current;
      const factor = e.deltaY > 0 ? 0.9 : 1.1;
      const newZ = Math.min(3, Math.max(0.1, oldZ * factor));
      const scale = newZ / oldZ;
      setZoom(newZ);
      setPan({ x: mx - (mx - panRef.current.x) * scale, y: my - (my - panRef.current.y) * scale });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const spawnTui = async (dir: string, role: string) => {
    setTuiErr("");
    try {
      const id = await api.tuiSpawn(dir, 90, 28);
      const cascade = (panes.length % 5) * 32 + 8;
      setPanes((p) => [...p, { id, dir, pos: { x: cascade, y: cascade } }]);
      setSelectedTui(id);
      if (role.trim()) setTuiRoles((m) => new Map(m).set(id, role.trim()));
      setSpawnOpen(false);
    } catch (e) { setTuiErr(`tui spawn failed: ${String(e)}`); }
  };

  const toggleSelect = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const connectTogether = () => {
    const ids = [...selected];
    if (ids.length < 2) return;
    const next = new Map<string, api.WorkflowEdge>();
    tuiEdges.forEach((e) => { if (!(selected.has(e.from) && selected.has(e.to))) next.set(e.id, e); });
    for (let i = 0; i < ids.length; i++) {
      for (let j = i + 1; j < ids.length; j++) {
        const id = `fc-${ids[i]}-${ids[j]}`;
        next.set(id, { id, from: ids[i], to: ids[j], from_port: "out", to_port: "in", condition: null, loop_edge: true });
      }
    }
    setTuiEdges([...next.values()]);
  };

  const clearSelection = () => { setSelected(new Set()); setSelectMode(false); };

  const selectedPane = selectedTui ? panes.find((x) => x.id === selectedTui) : null;
  const selectedRole = selectedPane ? (tuiRoles.get(selectedPane.id) || "") : "";
  const selectedEdges = selectedPane ? tuiEdges.filter((e) => e.from === selectedPane.id || e.to === selectedPane.id) : [];

  // Convert screen coords to canvas coords
  const screenToCanvas = useCallback((sx: number, sy: number) => {
    const el = canvasRef.current;
    if (!el) return { x: 0, y: 0 };
    const rect = el.getBoundingClientRect();
    return { x: (sx - rect.left - panRef.current.x) / zoomRef.current, y: (sy - rect.top - panRef.current.y) / zoomRef.current };
  }, []);

  const deleteLabel = (id: string) => setLabels((ls) => ls.filter((l) => l.id !== id));

  return (
    <div style={{ display: "flex", height: "100vh", overflow: "hidden" }}>
      {/* canvas + toolbar */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
        {/* toolbar */}
        <div style={{ display: "flex", alignItems: "center", gap: 2, background: "var(--panel)", borderBottom: "1px solid var(--line)", padding: "4px 10px", zIndex: 120, minHeight: 32 }}>
          <span className="mono" style={{ fontSize: 9, color: "var(--muted)", marginRight: 6 }}>{panes.length} TUI</span>
          <button className="action" style={{ fontSize: 10, padding: "3px 8px", fontWeight: 700 }} onClick={() => setSpawnOpen(true)} title="spawn terminal">+ TUI</button>
          <div style={{ width: 1, height: 14, background: "var(--line)", margin: "0 4px" }} />
          <button className="ghost" style={{ fontSize: 13, padding: "2px 8px", background: tool === "select" ? "rgba(255,255,255,0.06)" : undefined, borderColor: tool === "select" ? "var(--cyan)" : undefined, color: tool === "select" ? "var(--cyan)" : undefined }} onClick={() => { setTool("select"); setSelectMode(false); clearSelection(); }} title="select & move">↗</button>
          <button className="ghost" style={{ fontSize: 13, padding: "2px 8px", background: tool === "text" ? "rgba(255,255,255,0.06)" : undefined, borderColor: tool === "text" ? "var(--cyan)" : undefined, color: tool === "text" ? "var(--cyan)" : undefined }} onClick={() => { setTool("text"); setSelectMode(false); clearSelection(); }} title="text label">T</button>
          <button className="ghost" style={{ fontSize: 13, padding: "2px 8px", background: tool === "delete" ? "rgba(255,255,255,0.06)" : undefined, borderColor: tool === "delete" ? "var(--oom)" : undefined, color: tool === "delete" ? "var(--oom)" : undefined }} onClick={() => { setTool("delete"); setSelectMode(false); clearSelection(); }} title="delete">✕</button>
          <div style={{ width: 1, height: 14, background: "var(--line)", margin: "0 4px" }} />
          <button className="ghost" style={{ fontSize: 13, padding: "2px 8px", borderColor: selectMode ? "var(--magenta)" : undefined, color: selectMode ? "var(--magenta)" : undefined }} onClick={() => { if (selectMode) { setTool("select"); clearSelection(); } else { setTool("select"); setSelectMode(true); } }} title="multi-select">{selectMode ? "●" : "◉"}</button>
          {selected.size >= 2 && selectMode && (
            <button className="ghost" style={{ fontSize: 10, padding: "3px 8px", borderColor: "var(--pass)", color: "var(--pass)" }} onClick={() => { connectTogether(); clearSelection(); }}>⛓ ({selected.size})</button>
          )}
          {/* tool hint */}
          {tool === "text" && <span className="dim" style={{ fontSize: 9, marginLeft: 6 }}>click to place · dbl-click to edit</span>}
          {tool === "delete" && <span style={{ fontSize: 9, marginLeft: 6, color: "var(--oom)" }}>click to remove</span>}
          {selectMode && selected.size > 0 && (
            <div style={{ display: "flex", gap: 4, alignItems: "center", marginLeft: 6, fontSize: 9, color: "var(--magenta)" }}>
              {[...selected].slice(0, 4).map((id) => <span key={id} className="mono">{tuiRoles.get(id) || id.slice(0, 6)}</span>)}
              {selected.size > 4 && <span className="dim">+{selected.size - 4}</span>}
              {selected.size >= 2 && <button className="ghost" style={{ fontSize: 9, padding: "1px 6px", borderColor: "var(--pass)", color: "var(--pass)" }} onClick={() => { connectTogether(); clearSelection(); }}>⛓</button>}
            </div>
          )}
          {selectedTui && (
            <button className="ghost" style={{ fontSize: 10, padding: "2px 8px", marginLeft: "auto", borderColor: drawerOpen ? "var(--cyan)" : undefined, color: drawerOpen ? "var(--cyan)" : undefined }} onClick={() => setDrawerOpen((o) => !o)} title="toggle inspector panel">⚙</button>
          )}
        </div>

        {/* canvas area */}
        <div
          ref={canvasRef}
          onClick={(e) => {
            setCtxMenu(null);
            if (tool === "text") {
              const pos = screenToCanvas(e.clientX, e.clientY);
              const id = `label-${Date.now().toString(36)}`;
              setLabels((ls) => [...ls, { id, pos, text: "label" }]);
              setEditingLabel(id);
              return;
            }
            if (tool === "delete") {
              setSelectedTui(null);
              return;
            }
            if (!selectMode) setSelectedTui(null);
          }}
          onPointerDown={(e) => {
            if (e.button === 1 || (e.button === 0 && spaceHeld.current)) {
              e.preventDefault();
              const sx = e.clientX, sy = e.clientY;
              const orig = { ...panRef.current };
              const move = (ev: PointerEvent) => setPan({ x: orig.x + (ev.clientX - sx), y: orig.y + (ev.clientY - sy) });
              const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
              window.addEventListener("pointermove", move);
              window.addEventListener("pointerup", up);
            }
          }}
          style={{ flex: 1, overflow: "hidden", position: "relative", background: "var(--panel-2)", cursor: "default" }}
        >
          <div style={{ position: "absolute", inset: 0, overflow: "hidden" }}>
            {/* dot grid */}
            <div style={{ position: "absolute", inset: 0, backgroundImage: "radial-gradient(circle, rgba(255,255,255,0.06) 1px, transparent 1px)", backgroundSize: `${24 * zoom}px ${24 * zoom}px`, backgroundPosition: `${pan.x % (24 * zoom)}px ${pan.y % (24 * zoom)}px`, pointerEvents: "none", zIndex: 0 }} />
            <div style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`, transformOrigin: "0 0", position: "absolute", top: 0, left: 0, minWidth: "100%", minHeight: "100%" }}>
              {/* loopable edges */}
              <svg style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", pointerEvents: "none" }}>
                {tuiEdges.map((e) => {
                  const a = panes.find((pp) => pp.id === e.from);
                  const b = panes.find((pp) => pp.id === e.to);
                  if (!a || !b) return null;
                  const x1 = a.pos.x + 390, y1 = a.pos.y + 240, x2 = b.pos.x + 390, y2 = b.pos.y + 240;
                  const col = e.loop_edge ? "var(--magenta)" : "var(--dim2)";
                  return <g key={e.id}><path d={`M ${x1} ${y1} C ${x1} ${y1+40}, ${x2} ${y2-40}, ${x2} ${y2}`} fill="none" stroke={col} strokeWidth={e.loop_edge ? 2 : 1.2} strokeDasharray={e.loop_edge ? "6 4" : undefined} /><text x={(x1+x2)/2} y={(y1+y2)/2 - 6} textAnchor="middle" fontSize={9} fill={col}>{e.loop_edge ? "⟲ loop" : ""}</text></g>;
                })}
              </svg>
              {/* terminals */}
              {panes.map((p) => (
                <TuiWindow key={p.id} pane={{ id: p.id, dir: p.dir }} pos={p.pos} selected={(selectedTui === p.id) || (selectMode && selected.has(p.id))} role={tuiRoles.get(p.id)} connecting={connectingFrom === p.id} onStartConnect={(id) => setConnectingFrom(id)} zoom={zoom} onSelect={(id) => {
                  if (tool === "delete") {
                    void api.tuiStop(id);
                    setPanes((arr) => arr.filter((x) => x.id !== id));
                    setTuiRoles((m) => { const n = new Map(m); n.delete(id); return n; });
                    setTuiEdges((ee) => ee.filter((e) => e.from !== id && e.to !== id));
                    if (selectedTui === id) setSelectedTui(null);
                    if (connectingFrom === id) setConnectingFrom(null);
                    return;
                  }
                  if (selectMode) { toggleSelect(id); return; }
                  if (connectingFrom && connectingFrom !== id) {
                    const exists = tuiEdges.some((ee) => ee.from === connectingFrom && ee.to === id);
                    if (!exists) {
                      const edge: api.WorkflowEdge = { id: `te-${Date.now().toString(36)}`, from: connectingFrom, to: id, from_port: "out", to_port: "in", condition: null, loop_edge: false };
                      setTuiEdges((ee) => [...ee, edge]);
                    }
                    setConnectingFrom(null);
                  }
                  setSelectedTui(id);
                }} onContextMenu={(e, id) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY, nodeId: id }); setSelectedTui(id); }} onPos={(pos) => setPanes((arr) => arr.map((x) => x.id === p.id ? { ...x, pos } : x))} onDismiss={(id) => {
                  void api.tuiStop(id);
                  setPanes((arr) => arr.filter((x) => x.id !== id));
                  setTuiRoles((m) => { const n = new Map(m); n.delete(id); return n; });
                  setTuiEdges((ee) => ee.filter((e) => e.from !== id && e.to !== id));
                  if (selectedTui === id) setSelectedTui(null);
                  if (connectingFrom === id) setConnectingFrom(null);
                }} />
              ))}
              {/* text labels */}
              {labels.map((l) => (
                <div
                  key={l.id}
                  onPointerDown={(e) => {
                    if (tool === "delete") { deleteLabel(l.id); e.stopPropagation(); return; }
                    if (editingLabel) return;
                    e.stopPropagation();
                    const startX = e.clientX, startY = e.clientY;
                    const origPos = { ...l.pos };
                    const move = (ev: PointerEvent) => {
                      const dx = (ev.clientX - startX) / zoomRef.current;
                      const dy = (ev.clientY - startY) / zoomRef.current;
                      setLabels((ls) => ls.map((ll) => ll.id === l.id ? { ...ll, pos: { x: origPos.x + dx, y: origPos.y + dy } } : ll));
                    };
                    const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
                    window.addEventListener("pointermove", move);
                    window.addEventListener("pointerup", up);
                  }}
                  onDoubleClick={(e) => { e.stopPropagation(); setEditingLabel(l.id); }}
                  style={{ position: "absolute", left: l.pos.x, top: l.pos.y, cursor: tool === "delete" ? "not-allowed" : "grab", userSelect: "none", zIndex: 5 }}
                >
                  {editingLabel === l.id ? (
                    <input
                      autoFocus
                      value={l.text}
                      onChange={(e) => setLabels((ls) => ls.map((ll) => ll.id === l.id ? { ...ll, text: e.target.value } : ll))}
                      onBlur={() => setEditingLabel(null)}
                      onKeyDown={(e) => { if (e.key === "Enter") setEditingLabel(null); }}
                      onClick={(e) => e.stopPropagation()}
                      style={{ background: "rgba(11,11,11,0.8)", border: "1px solid var(--cyan)", color: "var(--text)", padding: "4px 8px", fontSize: 12, fontFamily: "inherit", borderRadius: 4, outline: "none", minWidth: 60 }}
                    />
                  ) : (
                    <div style={{ background: "rgba(11,11,11,0.7)", border: "1px solid var(--dim2)", color: "var(--text)", padding: "4px 8px", fontSize: 12, borderRadius: 4, whiteSpace: "nowrap", backdropFilter: "blur(4px)" }}>
                      {l.text || "empty"}
                    </div>
                  )}
                </div>
              ))}
              {/* empty state */}
              {panes.length === 0 && (
                <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", pointerEvents: "none" }}>
                  <div className="dim" style={{ fontSize: 11, textAlign: "center" }}>
                    click <b>+ TUI</b> to spawn an opencode session<br />
                    pick a model inside the terminal via <code>/model</code><br />
                    use the toolbar: select, text labels, delete, connect
                  </div>
                </div>
              )}
              {/* connecting indicator */}
              {connectingFrom && <div style={{ position: "absolute", top: 8, left: "50%", transform: "translateX(-50%)", background: "var(--magenta)", color: "#000", fontSize: 10, padding: "4px 10px", borderRadius: 4, pointerEvents: "none" }}>connecting from {tuiRoles.get(connectingFrom) || connectingFrom.slice(0,8)} — click target to wire · Esc</div>}
            </div>
          </div>

          {/* context menu */}
          {ctxMenu && (
            <div style={{ position: "fixed", left: ctxMenu.x, top: ctxMenu.y, background: "var(--panel)", border: "1px solid var(--line-bright)", borderRadius: 6, boxShadow: "0 8px 24px rgba(0,0,0,0.5)", zIndex: 99, fontSize: 11, minWidth: 160 }} onClick={() => setCtxMenu(null)}>
              <div style={{ padding: "6px 10px", cursor: "pointer" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); void (async () => { const id2 = await api.tuiSpawn("/home/deviant/Projects/cyberdeck", 90, 28); const pp = panes.find((pp) => pp.id === id); const pos = pp ? { x: pp.pos.x + 24, y: pp.pos.y + 24 } : { x: 24, y: 24 }; setPanes((a) => [...a, { id: id2, dir: "/home/deviant/Projects/cyberdeck", pos }]); setSelectedTui(id2); })(); void id; }}>Duplicate</div>
              <div style={{ padding: "6px 10px", cursor: "pointer", color: "var(--oom)" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); void api.tuiStop(id); setPanes((a) => a.filter((x) => x.id !== id)); if (selectedTui === id) setSelectedTui(null); }}>Close</div>
              <div style={{ padding: "6px 10px", cursor: "pointer", borderTop: "1px solid var(--line)" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); setSelectedTui(id); }}>Select</div>
            </div>
          )}

          {/* workspace panel — collapsible left drawer */}
          <div style={{ position: "absolute", top: 0, left: 0, bottom: 0, zIndex: 130, display: "flex", pointerEvents: "none" }}>
            <div
              onClick={() => setWsPanelOpen((v) => !v)}
              style={{ pointerEvents: "auto", width: 24, display: "flex", alignItems: "center", justifyContent: "center", background: "rgba(11,11,11,0.85)", border: "1px solid var(--line)", borderLeft: "none", borderRadius: "0 4px 4px 0", cursor: "pointer", fontSize: 10, color: "var(--muted)", writingMode: "vertical-lr", letterSpacing: 1, backdropFilter: "blur(4px)", userSelect: "none" }}
              title={wsPanelOpen ? "collapse workspaces" : "saved workspaces"}
            >
              {wsPanelOpen ? "◁" : "▷"}
            </div>
            {wsPanelOpen && (
              <div style={{ pointerEvents: "auto", width: 240, background: "var(--panel)", borderRight: "1px solid var(--line)", display: "flex", flexDirection: "column", overflow: "hidden", boxShadow: "4px 0 24px rgba(0,0,0,0.3)" }}>
                <div style={{ padding: "10px 10px 8px", borderBottom: "1px solid var(--line)" }}>
                  <div style={{ fontSize: 11, fontWeight: 700, color: "var(--muted)", letterSpacing: 0.6, marginBottom: 8 }}>WORKSPACES</div>
                  <div style={{ display: "flex", gap: 4 }}>
                    <input value={wsName} onChange={(e) => setWsName(e.target.value)} placeholder="name (optional)" onKeyDown={(e) => { if (e.key === "Enter") saveWorkspace(); }} style={{ flex: 1, background: "var(--panel-2)", border: "1px solid var(--line)", color: "var(--text)", padding: "4px 6px", fontSize: 10, borderRadius: 4 }} />
                    <button className="action" style={{ fontSize: 9, padding: "4px 8px" }} onClick={saveWorkspace}>SAVE</button>
                  </div>
                </div>
                <div style={{ flex: 1, overflow: "auto", padding: 4 }}>
                  {workspaces.length === 0 ? (
                    <div className="dim" style={{ fontSize: 10, padding: 12, textAlign: "center" }}>no saved workspaces yet</div>
                  ) : workspaces.map((ws) => (
                    <div key={ws.id} style={{ padding: "6px 8px", borderRadius: 4, cursor: "pointer", border: "1px solid transparent", marginBottom: 2 }} onMouseEnter={(e) => { e.currentTarget.style.background = "var(--panel-2)"; e.currentTarget.style.borderColor = "var(--line)"; }} onMouseLeave={(e) => { e.currentTarget.style.background = "transparent"; e.currentTarget.style.borderColor = "transparent"; }}>
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                        <span style={{ fontSize: 11, fontWeight: 600, color: "var(--text)" }}>{ws.name}</span>
                        <button className="ghost" style={{ fontSize: 8, padding: "1px 4px", color: "var(--oom)" }} onClick={(e) => { e.stopPropagation(); deleteWorkspace(ws.id); }} title="delete">✕</button>
                      </div>
                      <div className="dim" style={{ fontSize: 9, marginTop: 2 }}>
                        {ws.panes.length} terminal{ws.panes.length !== 1 ? "s" : ""} · {ws.labels.length} label{ws.labels.length !== 1 ? "s" : ""} · {ws.edges.length} edge{ws.edges.length !== 1 ? "s" : ""}
                      </div>
                      <div className="dim" style={{ fontSize: 8, marginTop: 1 }}>{new Date(ws.savedAt).toLocaleString()}</div>
                      <button className="ghost" style={{ fontSize: 9, padding: "2px 6px", marginTop: 4, width: "100%" }} onClick={(e) => { e.stopPropagation(); loadWorkspace(ws); }}>LOAD</button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>

          {/* error toast */}
          {tuiErr && (
            <div style={{ position: "absolute", bottom: 48, left: "50%", transform: "translateX(-50%)", background: "rgba(248,81,73,0.1)", border: "1px solid rgba(248,81,73,0.3)", color: "var(--oom)", padding: "6px 10px", fontSize: 11, borderRadius: 4, zIndex: 110 }}>{tuiErr}</div>
          )}

          {/* zoom — bottom left */}
          <div style={{ position: "absolute", bottom: 8, left: 6, zIndex: 110, display: "flex", flexDirection: "column", alignItems: "center", gap: 2, background: "rgba(11,11,11,0.85)", border: "1px solid var(--line)", borderRadius: 6, padding: "6px 4px", backdropFilter: "blur(4px)" }}>
            <button className="ghost" style={{ fontSize: 11, padding: "0", lineHeight: 1, color: "var(--text)", width: 16, height: 16 }} onClick={() => setZoom((z) => Math.min(3, z * 1.25))} title="zoom in">+</button>
            <div style={{ position: "relative", width: 2, height: 60, background: "var(--line)", borderRadius: 1, cursor: "pointer" }} onClick={(e) => {
              const rect = e.currentTarget.getBoundingClientRect();
              const pct = 1 - (e.clientY - rect.top) / rect.height;
              const z = 0.1 + pct * 2.9;
              setZoom(Math.min(3, Math.max(0.1, z)));
            }}>
              <div style={{ position: "absolute", left: -3, width: 8, height: 8, borderRadius: "50%", background: "var(--cyan)", top: `${(1 - (zoom - 0.1) / 2.9) * 100}%`, transform: "translateY(-50%)", cursor: "grab" }} onPointerDown={(e) => {
                e.preventDefault();
                const line = e.currentTarget.parentElement!;
                const move = (ev: PointerEvent) => {
                  const rect = line.getBoundingClientRect();
                  const pct = 1 - Math.min(1, Math.max(0, (ev.clientY - rect.top) / rect.height));
                  setZoom(Math.min(3, Math.max(0.1, 0.1 + pct * 2.9)));
                };
                const up = () => { window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); };
                window.addEventListener("pointermove", move);
                window.addEventListener("pointerup", up);
              }} />
            </div>
            <button className="ghost" style={{ fontSize: 11, padding: "0", lineHeight: 1, color: "var(--text)", width: 16, height: 16 }} onClick={() => setZoom((z) => Math.max(0.1, z / 1.25))} title="zoom out">−</button>
            <span className="mono" style={{ fontSize: 7, color: "var(--dim2)", marginTop: 1 }}>{Math.round(zoom * 100)}%</span>
          </div>

          {/* minimap — bottom right */}
          <div style={{ position: "absolute", bottom: 8, right: 8, width: 180, height: 120, zIndex: 110, background: "rgba(11,11,11,0.85)", border: "1px solid var(--line)", borderRadius: 6, overflow: "hidden", backdropFilter: "blur(4px)", cursor: "pointer" }} onPointerDown={(e) => {
            const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
            const mx = e.clientX - rect.left;
            const my = e.clientY - rect.top;
            const allPos = panes.map((p) => ({ x: p.pos.x, y: p.pos.y, w: 780, h: 480 }));
            if (allPos.length === 0) return;
            const minX = Math.min(...allPos.map((a) => a.x)) - 50;
            const minY = Math.min(...allPos.map((a) => a.y)) - 50;
            const maxX = Math.max(...allPos.map((a) => a.x + a.w)) + 50;
            const maxY = Math.max(...allPos.map((a) => a.y + a.h)) + 50;
            const cw = maxX - minX || 1;
            const ch = maxY - minY || 1;
            const s = Math.min(180 / cw, 120 / ch);
            const canvasX = mx / s + minX;
            const canvasY = my / s + minY;
            const viewEl = canvasRef.current;
            if (viewEl) {
              const vw = viewEl.clientWidth;
              const vh = viewEl.clientHeight;
              setPan({ x: vw / 2 - canvasX * zoomRef.current, y: vh / 2 - canvasY * zoomRef.current });
            }
          }}>
            {(() => {
              if (panes.length === 0) return <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", fontSize: 8, color: "var(--dim2)" }}>no terminals</div>;
              const allPos = panes.map((p) => ({ x: p.pos.x, y: p.pos.y, w: 780, h: 480 }));
              const minX = Math.min(...allPos.map((a) => a.x)) - 50;
              const minY = Math.min(...allPos.map((a) => a.y)) - 50;
              const maxX = Math.max(...allPos.map((a) => a.x + a.w)) + 50;
              const maxY = Math.max(...allPos.map((a) => a.y + a.h)) + 50;
              const cw = maxX - minX || 1;
              const ch = maxY - minY || 1;
              const s = Math.min(180 / cw, 120 / ch);
              const viewEl = canvasRef.current;
              const vw = viewEl ? viewEl.clientWidth : 800;
              const vh = viewEl ? viewEl.clientHeight : 600;
              const vLeft = -panRef.current.x / zoomRef.current;
              const vTop = -panRef.current.y / zoomRef.current;
              const vW = vw / zoomRef.current;
              const vH = vh / zoomRef.current;
              return (
                <svg width={180} height={120} style={{ position: "absolute", inset: 0 }}>
                  {allPos.map((a, i) => (
                    <rect key={i} x={(a.x - minX) * s} y={(a.y - minY) * s} width={a.w * s} height={a.h * s} rx={2} fill={selected.has(panes[i]?.id) ? "rgba(210,153,34,0.5)" : "rgba(0,255,170,0.25)"} stroke={selected.has(panes[i]?.id) ? "var(--magenta)" : "rgba(0,255,170,0.4)"} strokeWidth={0.5} />
                  ))}
                  <rect x={(vLeft - minX) * s} y={(vTop - minY) * s} width={vW * s} height={vH * s} fill="none" stroke="rgba(255,255,255,0.5)" strokeWidth={1} strokeDasharray="3 2" rx={1} />
                </svg>
              );
            })()}
          </div>
        </div>
      </div>

      {/* right drawer — terminal inspector */}
      {drawerOpen && selectedPane && (
        <div style={{ width: 360, flex: "none", overflow: "auto", display: "flex", flexDirection: "column", borderLeft: "1px solid var(--line)", background: "var(--bg)", padding: 10, gap: 10 }}>
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
            <span style={{ fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>TERMINAL — {selectedPane.id.slice(0, 8)}</span>
            <button className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} onClick={() => setSelectedTui(null)}>✕</button>
          </div>
          <label style={{ fontSize: 11, color: "var(--muted)" }}>
            role — click badge or type to assign (for loops)
            <input list={`roles-drawer-${selectedPane.id}`} value={selectedRole} onChange={(e) => setTuiRoles((m) => { const n = new Map(m); const v = e.target.value.trim(); if (v) n.set(selectedPane.id, v); else n.delete(selectedPane.id); return n; })} placeholder="primary-developer — or type custom" style={{ width: "100%", background: "var(--panel)", border: "1px solid var(--line)", color: "var(--text)", padding: "4px 6px", fontSize: 11, marginTop: 4 }} />
            <datalist id={`roles-drawer-${selectedPane.id}`}><option value="primary-developer" /><option value="architecture-reviewer" /><option value="human" /></datalist>
          </label>
          {selectedRole === "human" && <div className="dim" style={{ fontSize: 9, color: "var(--magenta)" }}>human gate — this terminal pauses for your approval after the loop is satisfied</div>}
          <div className="mono dim" style={{ fontSize: 10 }}>dir: {selectedPane.dir}</div>
          <div className="mono dim" style={{ fontSize: 10 }}>model: inside terminal via <code>/model</code></div>
          <div style={{ borderTop: "1px solid var(--line)", paddingTop: 8 }}>
            <div style={{ fontSize: 11, fontWeight: 600, color: "var(--muted)", marginBottom: 4 }}>CONNECTIONS — {selectedEdges.length ? `${selectedEdges.length} edge${selectedEdges.length>1?"s":""}` : "no connections"} {panes.length > 1 && <span className="dim">· click ● on another terminal to connect (loopable)</span>}</div>
            {selectedEdges.length === 0 ? <div className="dim" style={{ fontSize: 10 }}>give this terminal a role, then click its ● handle and pick a target terminal to wire. Use <code>loop</code> for developer→reviewer→developer cycle, and a final edge to a <code>human</code> terminal for approval.</div> : selectedEdges.map((e) => {
              const other = e.from === selectedPane.id ? e.to : e.from;
              const dir = e.from === selectedPane.id ? "→" : "←";
              const otherRole = tuiRoles.get(other) || other.slice(0,8);
              return (
                <div key={e.id} className="row" style={{ gap: 6, alignItems: "center", padding: "4px 0", borderBottom: "1px solid var(--line)", flexWrap: "wrap" }}>
                  <span className="mono" style={{ fontSize: 10 }}>{dir} {otherRole}</span>
                  <input placeholder="condition e.g. contains:APPROVED" value={e.condition || ""} onChange={(ev) => setTuiEdges((ee) => ee.map((x) => x.id === e.id ? { ...x, condition: ev.target.value || null } : x))} style={{ flex: 1, minWidth: 120, background: "var(--panel-2)", border: "1px solid var(--line)", color: "var(--text)", padding: "2px 4px", fontSize: 9 }} title="contains:APPROVED / not_contains:CHANGES_REQUESTED / always" />
                  <label className="row" style={{ gap: 4, fontSize: 10 }}><input type="checkbox" checked={!!e.loop_edge} onChange={(ev) => setTuiEdges((ee) => ee.map((x) => x.id === e.id ? { ...x, loop_edge: ev.target.checked } : x))} /> loop</label>
                  <button className="ghost" style={{ fontSize: 9, padding: "2px 4px", color: "var(--oom)" }} onClick={() => setTuiEdges((ee) => ee.filter((x) => x.id !== e.id))}>✕</button>
                </div>
              );
            })}
          </div>
          <div className="row" style={{ gap: 6 }}>
            <button className="ghost" style={{ fontSize: 11, flex: 1 }} onClick={() => setConnectingFrom(selectedPane.id)} title="wire this terminal to another — loopable edge">CONNECT</button>
          </div>
          {(() => {
            const hasHuman = [...tuiRoles.values()].includes("human");
            const hasLoop = tuiEdges.some((e) => e.loop_edge);
            if (!hasHuman || !hasLoop) return <div className="dim" style={{ fontSize: 9, borderTop: "1px solid var(--line)", paddingTop: 6 }}>Tip: assign one terminal <code>human</code>, wire <code>developer → reviewer (loop)</code> with condition <code>contains:CHANGES</code> and <code>reviewer → human</code> with <code>contains:APPROVED</code> — loop runs until reviewer is satisfied, then pauses for your approval.</div>;
            return null;
          })()}
          <div className="dim" style={{ fontSize: 10, borderTop: "1px solid var(--line)", paddingTop: 8 }}>Each TUI runs stock opencode — accept/deny inside the terminal. Roles + loop edges are extra when clicked. Cloud model via <code>/model</code> in the TUI.</div>
          <div className="row" style={{ gap: 6 }}>
            <button className="ghost" style={{ fontSize: 11, color: "var(--oom)", flex: 1 }} onClick={() => { void api.tuiStop(selectedPane.id); setPanes((a) => a.filter((x) => x.id !== selectedPane.id)); setTuiRoles((m) => { const n = new Map(m); n.delete(selectedPane.id); return n; }); setTuiEdges((ee) => ee.filter((e) => e.from !== selectedPane.id && e.to !== selectedPane.id)); setSelectedTui(null); }}>CLOSE TERMINAL</button>
            <button className="ghost" style={{ fontSize: 11, flex: 1 }} onClick={() => setSelectedTui(null)}>DISMISS</button>
          </div>
        </div>
      )}

      {/* spawn dialog */}
      {spawnOpen && (
        <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.6)", zIndex: 150, display: "flex", alignItems: "center", justifyContent: "center" }} onClick={() => setSpawnOpen(false)}>
          <div onClick={(e) => e.stopPropagation()} style={{ background: "var(--panel)", border: "1px solid var(--line-bright)", width: 480, maxWidth: "92vw", padding: 16, borderRadius: 8, boxShadow: "0 20px 60px rgba(0,0,0,0.6)" }}>
            <div className="row" style={{ justifyContent: "space-between", alignItems: "center", marginBottom: 12 }}>
              <h3 style={{ margin: 0, fontSize: 13, color: "var(--text)" }}>+ TERMINAL</h3>
              <button className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} onClick={() => setSpawnOpen(false)}>✕</button>
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 10, fontSize: 11 }}>
              <label style={{ display: "flex", flexDirection: "column", gap: 4, color: "var(--muted)" }}>
                folder
                <div style={{ display: "flex", gap: 6 }}>
                  <input value={spawnDir} onChange={(e) => setSpawnDir(e.target.value)} placeholder="/path/to/project" style={{ flex: 1, background: "var(--panel-2)", border: "1px solid var(--line)", color: "var(--text)", padding: "6px 10px", fontSize: 11, fontFamily: "monospace" }} />
                  <button className="ghost" style={{ fontSize: 10, padding: "6px 10px" }} onClick={async () => { const dir = await open({ directory: true, title: "Select project folder" }); if (dir) setSpawnDir(dir); }}>browse</button>
                </div>
              </label>
              <label style={{ display: "flex", flexDirection: "column", gap: 4, color: "var(--muted)" }}>
                role (for loops)
                <input value={spawnRole} onChange={(e) => setSpawnRole(e.target.value)} placeholder="primary-developer, reviewer, human… (optional)" style={{ background: "var(--panel-2)", border: "1px solid var(--line)", color: "var(--text)", padding: "6px 10px", fontSize: 11 }} />
              </label>
              <div className="row" style={{ gap: 8, marginTop: 4 }}>
                <button className="action" style={{ flex: 1 }} onClick={() => spawnTui(spawnDir, spawnRole)}>SPAWN</button>
                <button className="ghost" style={{ flex: 1 }} onClick={() => { setSpawnOpen(false); setSpawnDir("/home/deviant/Projects/cyberdeck"); setSpawnRole(""); }}>CANCEL</button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
