import { useCallback, useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import * as api from "../api";
import { latestBySlot, slotKey } from "../lib/portmap";
import TuiWindow from "../components/TuiWindow";
import LoadoutEditor, { defaultProfile } from "./LoadoutEditor";

const ENGINE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
  { engine: "Ollama", host: "127.0.0.1", port: 11434 },
];

function isLocalModel(ref: string): boolean {
  const r = ref.toLowerCase();
  if (r.startsWith("openrouter/") || r.startsWith("anthropic/") || r.startsWith("openai/")) return false;
  if (r.startsWith("ollama/")) return false;
  return true;
}

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
  const [prompt, setPrompt] = useState("");
  const [dir, setDir] = useState("/home/deviant/Projects/cyberdeck");
  const [auto, setAuto] = useState(false);
  const [loadout, setLoadout] = useState("");
  const [harnessModel, setHarnessModel] = useState("");
  const [customModel, setCustomModel] = useState("");
  const [ctx, setCtx] = useState(32768);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [status, setStatus] = useState<api.EngineStatus[]>([]);
  const [sessions, setSessions] = useState<{ id: string; prompt: string; log: string[]; running: boolean; model?: string }[]>([]);
  const [panes, setPanes] = useState<{ id: string; dir: string; pos: { x: number; y: number } }[]>([]);
  const cardPos = useRef<Map<string, { x: number; y: number }>>(new Map());
  const sessionCountRef = useRef(0);
  const [residents, setResidents] = useState<api.PortMapSlot[]>([]);
  const [benchBySlot, setBenchBySlot] = useState<Map<string, { tps: number; ctx: number; model: string; at: number }>>(() => new Map());
  const sessionsRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const runningSessionIds = useRef<Set<string>>(new Set());
  const active = profiles.find((p) => p.name === loadout) ?? null;

  const [selectedTui, setSelectedTui] = useState<string | null>(null);
  const [selectedNode, setSelectedNode] = useState<string | null>(null);
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; nodeId: string } | null>(null);

  // workflows (kept for bench + optional DAG, hidden by default for plain-terminal birds-eye)
  const [workflows, setWorkflows] = useState<api.Workflow[]>([]);
  const [selectedWf, setSelectedWf] = useState<string>("");
  const [showWorkflows, setShowWorkflows] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [runner, setRunner] = useState<"stateless" | "agentic" | "echo">("echo");
  const [wfDir, setWfDir] = useState("");
  const [kickoffTask, setKickoffTask] = useState("");
  const [wfMsg, setWfMsg] = useState("");
  const [bench, setBench] = useState<api.RoleBenchRow[]>([]);
  const [loopBench, setLoopBench] = useState<api.LoopBenchRow | null>(null);
  const [editing, setEditing] = useState<api.Profile | null>(null);
  const [modelPaths, setModelPaths] = useState<string[]>([]);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [harnessErr, setHarnessErr] = useState("");
  const [pending, setPending] = useState(false);
  const [tuiErr, setTuiErr] = useState("");
  // TUI roles + loopable edges — plain terminals become role-bound loop nodes without app-side model wiring
  const [tuiRoles, setTuiRoles] = useState<Map<string, string>>(new Map());
  const tuiRolesRef = useRef<Map<string, string>>(new Map());
  useEffect(() => { tuiRolesRef.current = tuiRoles; }, [tuiRoles]);
  const [tuiEdges, setTuiEdges] = useState<api.WorkflowEdge[]>([]);
  const [connectingFrom, setConnectingFrom] = useState<string | null>(null);
  const [humanGate, setHumanGate] = useState<{ code: string; from: string; to: string } | null>(null);
  const [wfTuiMap, setWfTuiMap] = useState<Map<string, string>>(new Map());
  const wfTuiMapRef = useRef<Map<string, string>>(new Map());
  useEffect(() => { wfTuiMapRef.current = wfTuiMap; }, [wfTuiMap]);
  const [activeWfNode, setActiveWfNode] = useState<string | null>(null);
  const [lastMessage, setLastMessage] = useState<string>("");
  const nodeOutputsRef = useRef<Map<string, string>>(new Map());
  useEffect(() => { const h = (e: KeyboardEvent) => { if (e.key === "Escape") setConnectingFrom(null); }; window.addEventListener("keydown", h); return () => window.removeEventListener("keydown", h); }, []);

  useEffect(() => { api.listModels().then((m) => setModelPaths(m.map((x) => x.path))).catch(() => {}); }, []);

  const loadWorkflows = useCallback(async () => {
    try {
      const wfs = await api.workflowList();
      setWorkflows(wfs);
      if (!selectedWf && wfs.length > 0) setSelectedWf(wfs[0].id);
      const h = await api.workflowHistory();
      // history is WfRunRow, not needed for plain mode but keep
      void h;
    } catch (e) { setWfMsg(String(e)); }
  }, [selectedWf]);

  useEffect(() => { void api.workflowSeed().then(() => void loadWorkflows()).catch(() => {}); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const refetchResidents = useCallback(async () => {
    const [slots, hist] = await Promise.all([api.portMapStatus("127.0.0.1"), api.benchHistory()]);
    setResidents(slots);
    setBenchBySlot(latestBySlot(hist));
  }, []);

  useEffect(() => {
    Promise.all(ENGINE_NODES.map((n) => api.engineStatus(n.engine, n.host, n.port).catch(() => null))).then((r) =>
      setStatus(r.filter(Boolean) as api.EngineStatus[])
    );
    refetchResidents();
    const t = window.setInterval(() => void refetchResidents(), 15000);
    return () => window.clearInterval(t);
  }, [refetchResidents]);

  useEffect(() => {
    const a = listen<{ id: string; prompt: string }>("opencode-started", (e) => {
      runningSessionIds.current.add(e.payload.id);
      setSessions((s) => {
        const pending = s.find((x) => x.id.startsWith("pending-") && x.running);
        if (pending) {
          const p = cardPos.current.get(pending.id);
          if (p) cardPos.current.set(e.payload.id, p);
          cardPos.current.delete(pending.id);
          return s.map((x) => x.id === pending.id ? { ...x, id: e.payload.id, log: [...x.log, `[deck] session ${e.payload.id} started`] } : x);
        }
        sessionCountRef.current += 1;
        return [...s, { id: e.payload.id, prompt: e.payload.prompt, log: [], running: true, model: "" }];
      });
    });
    const b = listen<{ session: string; stream: string; text: string }>("opencode-output", (e) => {
      setSessions((s) => {
        const target = s.find((x) => x.id === e.payload.session);
        if (target) return s.map((x) => x.id === e.payload.session ? { ...x, log: [...x.log, e.payload.text] } : x);
        const pending = s.find((x) => x.id.startsWith("pending-") && x.running);
        if (pending) {
          const p = cardPos.current.get(pending.id);
          if (p) cardPos.current.set(e.payload.session, p);
          cardPos.current.delete(pending.id);
          return s.map((x) => x.id === pending.id ? { ...x, id: e.payload.session, log: [...x.log, e.payload.text] } : x);
        }
        return s;
      });
    });
    const c = listen<{ session: string; code: number }>("opencode-done", (e) => {
      runningSessionIds.current.delete(e.payload.session);
      setSessions((s) => s.map((x) => (x.id === e.payload.session ? { ...x, running: false } : x)));
    });
    return () => {
      a.then((f) => f()); b.then((f) => f()); c.then((f) => f());
      runningSessionIds.current.forEach((id) => void api.opencodeStop(id));
    };
  }, []);

  useEffect(() => {
    let un: UnlistenFn[] = [];
    let mounted = true;
    (async () => {
      const ln = await listen<api.WfNodeEvt>("wf-node", (e) => {
        nodeOutputsRef.current.set(e.payload.node_id, e.payload.text || e.payload.error || "");
        setActiveWfNode(e.payload.node_id);
        setLastMessage(e.payload.text || e.payload.error || "");
        // mirror into the backing TUI so the canvas is actually connected — you see the TUIs activate one after another
        const paneId = wfTuiMap.get(e.payload.node_id) || tuiRolesRef.current.get(e.payload.node_id) || e.payload.node_id;
        const targetPane = panes.find((pp) => pp.id === paneId) || panes.find((pp) => pp.id === e.payload.node_id);
        if (targetPane) {
          const preview = (e.payload.text || e.payload.error || "").slice(0, 800);
          void api.tuiWrite(targetPane.id, Array.from(`\r\n[wf] ${e.payload.node_id}: ${preview}\r\n`, (c) => c.charCodeAt(0))).catch(() => {});
        }
        // also keep the dedicated "last message" pane in sync
        const msgPaneId = [...tuiRolesRef.current.entries()].find(([, r]) => r === "message")?.[0];
        if (msgPaneId) {
          const msgPane = panes.find((pp) => pp.id === msgPaneId);
          if (msgPane) void api.tuiWrite(msgPane.id, Array.from(`\r\n[${e.payload.node_id}] ${ (e.payload.text || "").slice(0, 600)}\r\n`, (c) => c.charCodeAt(0))).catch(() => {});
          setLastMessage(e.payload.text || e.payload.error || "");
        }
      });
      const ld = await listen<api.WfDoneEvt>("wf-done", (e) => {
        if (!mounted) return;
        setBusy(null);
        const isTuiLoop = e.payload.workflow_id?.startsWith("tui-loop-") || (e.payload as unknown as { run_id?: string }).run_id?.startsWith("tui-loop-");
        const hasHuman = [...tuiRolesRef.current.values()].includes("human");
        if (isTuiLoop && hasHuman) {
          // checker → human: surface the reviewer's actual output text, not a boilerplate summary.
          // Pick the last reviewer-ish node output (architecture-reviewer / reviewer), fallback to the last node.
          const entries = [...nodeOutputsRef.current.entries()];
          const reviewerEntry = entries.find(([id]) => id.toLowerCase().includes("review")) || entries[entries.length - 1];
          const reviewerText = reviewerEntry ? reviewerEntry[1] : "";
          const verdict = reviewerText.includes("APPROVED") ? "✓ APPROVED" : reviewerText.includes("CHANGES") ? "↺ CHANGES_REQUESTED" : "no verdict";
          const code = reviewerText
            ? `${reviewerText}\n\n— — —\nLoop: ${e.payload.nodes_ok} ok / ${e.payload.nodes_failed} failed · ${e.payload.status} · ${verdict} · iterations: ${e.payload.iterations ?? 1}`
            : `Loop finished: ${e.payload.nodes_ok} ok / ${e.payload.nodes_failed} failed · ${e.payload.status} · iterations: ${e.payload.iterations ?? 1}\n(no reviewer text captured — check reviewer output contains APPROVED)`;
          setHumanGate({ code, from: e.payload.workflow_id || "tui-loop", to: "human" });
        }
        setWfMsg(`${e.payload.status}: ${e.payload.nodes_ok} ok / ${e.payload.nodes_failed} failed`);
        loadWorkflows();
      });
      un = [ln, ld];
    })();
    return () => { mounted = false; un.forEach((u) => u()); };
  }, [loadWorkflows]);

  useEffect(() => {
    if (!selectedWf) { setBench([]); setLoopBench(null); return; }
    api.workflowPerRoleBench(selectedWf).then(setBench).catch(() => setBench([]));
    api.workflowLoopBench(selectedWf).then(setLoopBench).catch(() => setLoopBench(null));
  }, [selectedWf]);

  const runWorkflow = async () => {
    if (!selectedWf) return;
    nodeOutputsRef.current.clear();
    setBusy("run"); setWfMsg("");
    try { await api.workflowRun(selectedWf, runner, runner === "agentic" ? (wfDir || dir || "/home/deviant/Projects/cyberdeck") : (wfDir ? wfDir : null), null, kickoffTask.trim() || null); setWfMsg(`workflow '${selectedWf}' queued…`); } catch (e) { setWfMsg(String(e)); setBusy(null); }
  };
  const stopWorkflow = async () => {
    try {
      const hist = await api.workflowHistory();
      const runRows = hist.filter((r) => r.workflow_id === selectedWf && r.status === "Running");
      if (runRows.length === 0) { setWfMsg("no running run"); return; }
      await api.workflowStop(runRows[runRows.length - 1].id); setWfMsg("requested stop…");
    } catch (e) { setWfMsg(String(e)); }
  };

  const spawnTui = async () => {
    setTuiErr("");
    try {
      const id = await api.tuiSpawn("/home/deviant/Projects/cyberdeck", 90, 28);
      const cascade = (panes.length % 5) * 32 + 8;
      setPanes((p) => [...p, { id, dir: "/home/deviant/Projects/cyberdeck", pos: { x: cascade, y: cascade } }]);
      setSelectedTui(id);
    } catch (e) { setTuiErr(`tui spawn failed: ${String(e)}`); }
  };

  const runAgent = async () => {
    if (!prompt.trim()) return;
    // If a TUI is selected, address that TUI directly — no need to click inside the xterm.
    // This makes the loop addressable from the main bar (header task is for kickoff, bottom is for chat).
    if (selectedTui) {
      const target = panes.find((p) => p.id === selectedTui);
      if (target) {
        try { await api.tuiWrite(target.id, Array.from(prompt + "\r", (c) => c.charCodeAt(0))); setPrompt(""); setTimeout(() => inputRef.current?.focus(), 50); return; } catch (e) { setHarnessErr(String(e)); return; }
      }
    }
    if (selectedNode && wf) {
      const paneId = wfTuiMap.get(selectedNode);
      const target = paneId ? panes.find((p) => p.id === paneId) : null;
      if (target) {
        try { await api.tuiWrite(target.id, Array.from(prompt + "\r", (c) => c.charCodeAt(0))); setPrompt(""); return; } catch (e) { setHarnessErr(String(e)); return; }
      }
    }
    let chosen = (harnessModel === "__custom" ? customModel : harnessModel) || active?.model || "";
    const eng = chosen.split("/")[0]?.toLowerCase();
    if (!chosen) {
      const ftUp = status.find((s) => s.engine === "FreeToken")?.up;
      const llUp = status.find((s) => s.engine === "LlamaCpp")?.up;
      if (ftUp) chosen = "freetoken/qwen3.6-35b-a3b-nvfp4";
      else if (llUp) chosen = "llamacpp/qwen3.8-27b";
      else { setHarnessErr("No engine UP — spawn a terminal and pick a model inside opencode, or start one via loadout"); return; }
    }
    const snap = prompt;
    setHarnessErr(""); setPending(true);
    const optimisticId = `pending-${Date.now()}`;
    sessionCountRef.current += 1;
    const cascade = (sessionCountRef.current % 5) * 24;
    if (!cardPos.current.has(optimisticId)) cardPos.current.set(optimisticId, { x: cascade, y: cascade });
    setSessions((s) => [...s, { id: optimisticId, prompt: snap.slice(0, 120), log: [`[deck] spawning opencode ${chosen ? `-m ${chosen}` : ""} --dir ${dir} …`], running: true, model: chosen }]);
    const withTimeout = <T,>(p: Promise<T>, ms: number, msg: string) => Promise.race([p, new Promise<never>((_, rej) => setTimeout(() => rej(new Error(msg)), ms))]);
    try {
      await withTimeout(api.opencodeRun({ prompt: snap, dir, auto, model: chosen, engine: eng, ctx }), 15000, `opencode harness timed out — is ${eng} UP?`);
      setPrompt(""); setTimeout(() => inputRef.current?.focus(), 50);
    } catch (e) {
      const msg = String(e); setHarnessErr(msg);
      setSessions((s) => s.map((x) => x.id === optimisticId ? { ...x, log: [...x.log, `[harness error] ${msg}`], running: false } : x));
    } finally { setPending(false); }
  };
  const onKey = (e: React.KeyboardEvent) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); runAgent(); } };
  const edit = async (name: string) => { try { const full = await api.profileGet(name); if (full) { setEditing(full); setDrawerOpen(true); } } catch (e) { setWfMsg(String(e)); } };

  const wf = workflows.find((w) => w.id === selectedWf);
  const spawnedWfNodesRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!wf) return;
    const curIds = new Set(wf.nodes.map((n) => n.id));
    setWfTuiMap((m) => {
      const nn = new Map(m);
      for (const k of [...nn.keys()]) if (!curIds.has(k)) nn.delete(k);
      return nn;
    });
    wf.nodes.forEach((n) => {
      if (spawnedWfNodesRef.current.has(n.id)) return;
      spawnedWfNodesRef.current.add(n.id);
      void (async () => {
        try {
          const paneId = await api.tuiSpawn("/home/deviant/Projects/cyberdeck", 90, 28);
          setWfTuiMap((m) => { const nn = new Map(m); nn.set(n.id, paneId); return nn; });
          setPanes((pp) => [...pp, { id: paneId, dir: "/home/deviant/Projects/cyberdeck", pos: n.pos }]);
          setTuiRoles((mm) => { const nn2 = new Map(mm); nn2.set(paneId, n.role_id); return nn2; });
        } catch (err) { setTuiErr(String(err)); spawnedWfNodesRef.current.delete(n.id); }
      })();
    });
  }, [wf]);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "calc(100vh - 44px)", overflow: "hidden" }}>
      {/* header — plain-terminal birds-eye: + TERMINAL is primary, workflows optional */}
      <div style={{ display: "flex", gap: 8, alignItems: "center", padding: "8px 0 6px", flexWrap: "wrap", borderBottom: "1px solid var(--line)", marginBottom: 8 }}>
        <span style={{ fontSize: 11, fontWeight: 700, letterSpacing: 1, color: "var(--muted)" }}>WORKSPACE</span>
        <button className="action" style={{ fontSize: 11, padding: "6px 12px", fontWeight: 700 }} onClick={() => void spawnTui()} title="spawn plain opencode terminal — stock TUI, plug-and-play">+ TERMINAL</button>
        <button className="ghost" style={{ fontSize: 9, padding: "3px 6px", borderColor: showWorkflows ? "var(--magenta)" : undefined, color: showWorkflows ? "var(--magenta)" : undefined }} onClick={() => setShowWorkflows((v) => !v)} title="show workflow DAG — off by default for birds-eye terminals">{showWorkflows ? "◇ WORKFLOWS ON" : "◇ WORKFLOWS OFF"}</button>
        {wf && (
          <div style={{ display: "flex", gap: 6, alignItems: "center", marginLeft: 8, flexWrap: "wrap" }}>
            <select value={selectedWf} onChange={(e) => setSelectedWf(e.target.value)} style={{ background: "var(--panel-2)", color: "var(--text)", border: "1px solid var(--dim2)", borderRadius: 4, padding: "4px 8px", fontSize: 11, minWidth: 160 }}>
              <option value="">workflow — none</option>
              {workflows.map((w) => <option key={w.id} value={w.id}>{w.name} · {w.nodes.length}n</option>)}
            </select>
            <input value={kickoffTask} onChange={(e) => setKickoffTask(e.target.value)} placeholder="task — what should the loop build? (CrewAI inputs.task)" style={{ fontSize: 10, minWidth: 220, flex: "1 1 220px", background: "var(--panel-2)", color: "var(--text)", border: "1px solid var(--magenta)", borderRadius: 4, padding: "4px 8px" }} />
            <select value={runner} onChange={(e) => setRunner(e.target.value as "stateless" | "agentic" | "echo")} style={{ fontSize: 10, background: "var(--panel-2)", color: "var(--text)", border: "1px solid var(--dim2)", borderRadius: 4, padding: "3px 6px" }}>
              <option value="stateless">stateless</option><option value="agentic">agentic</option><option value="echo">echo (no-LLM demo)</option>
            </select>
            {runner === "agentic" && <input value={wfDir} placeholder="/path" onChange={(e) => setWfDir(e.target.value)} style={{ fontSize: 10, width: 120, background: "var(--panel-2)", color: "var(--text)", border: "1px solid var(--dim2)", borderRadius: 4, padding: "3px 6px" }} />}
            <button className="ghost" style={{ color: "var(--pass)", borderColor: "var(--pass)", fontSize: 10 }} onClick={runWorkflow} disabled={busy === "run"}>{busy === "run" ? "RUNNING…" : "▶ RUN"}</button>
            <button className="ghost" style={{ color: "var(--oom)", borderColor: "var(--oom)", fontSize: 10 }} onClick={stopWorkflow}>■ STOP</button>
          </div>
        )}
        <div style={{ marginLeft: "auto", display: "flex", gap: 6, alignItems: "center", fontSize: 10, color: "var(--dim2)" }}>
          <span>{panes.length} terminals</span>
          {sessions.length > 0 && <span>· {sessions.length} sessions</span>}
        </div>
      </div>

      {residents.some((r) => r.resident && r.profile) && (
        <div style={{ display: "flex", gap: 8, alignItems: "center", justifyContent: "center", flexWrap: "wrap", paddingBottom: 6, fontSize: 10 }}>
          {residents.filter((r) => r.resident && r.profile).map((r) => {
            const b = benchBySlot.get(slotKey(r.engine, r.port));
            return (
              <span key={r.engine} className="mono" style={{ display: "flex", alignItems: "center", gap: 4, color: "var(--text)" }}>
                <span style={{ width: 6, height: 6, borderRadius: "50%", background: r.state === "up" ? "var(--pass)" : r.state === "starting" ? "var(--warn)" : "var(--dim2)" }} />
                <span>{r.profile}</span>
                {b && <span style={{ color: "var(--cyan)" }}>{b.tps.toFixed(1)} tok/s</span>}
              </span>
            );
          })}
        </div>
      )}

      {/* canvas — left workflows list (always) + birds-eye plain terminals */}
      <div style={{ display: "flex", gap: 10, flex: 1, minHeight: 0, overflow: "hidden" }}>
        <div className="card" style={{ width: 200, flex: "none", display: "flex", flexDirection: "column", overflow: "hidden", fontSize: 11 }}>
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
            <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--muted)", margin: 0 }}>WORKFLOWS</h3>
            <button className="ghost" style={{ fontSize: 9, padding: "2px 6px" }} onClick={() => { void api.workflowSeed().then(() => void loadWorkflows()); }}>RESEED</button>
          </div>
          <div style={{ overflow: "auto", flex: 1 }}>
            {workflows.length === 0 ? <div className="dim" style={{ padding: 6 }}>no workflows — run CLI seed or + TERMINAL loop</div> : workflows.map((w) => (
              <div key={w.id} onClick={() => setSelectedWf(w.id)} style={{ padding: "5px 6px", cursor: "pointer", borderRadius: 4, background: selectedWf === w.id ? "var(--panel-2)" : "transparent" }}>
                <div style={{ color: "var(--cyan)", fontWeight: 600 }}>{w.name}</div>
                <div className="dim" style={{ fontSize: 9 }}>{w.id} · {w.nodes.length}n {w.edges.length}e</div>
              </div>
            ))}
          </div>
          <div className="dim" style={{ fontSize: 9, borderTop: "1px solid var(--line)", paddingTop: 6, marginTop: 6 }}>human-loop is there — select it to load its terminals (plain TUIs) as the workflow</div>
        </div>
        <div ref={sessionsRef} onClick={() => setCtxMenu(null)} style={{ flex: 1, overflow: "auto", position: "relative", background: "var(--panel-2)", borderRadius: 6, padding: 8, minWidth: 320 }}>
          <div style={{ position: "relative", minHeight: 520 }}>
            {/* loopable edges between plain terminals — birds-eye loop wiring */}
            <svg style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", pointerEvents: "none" }}>
              {tuiEdges.map((e) => {
                const a = panes.find((pp) => pp.id === e.from);
                const b = panes.find((pp) => pp.id === e.to);
                if (!a || !b) return null;
                const x1 = a.pos.x + 450, y1 = a.pos.y + 260, x2 = b.pos.x + 450, y2 = b.pos.y + 260;
                const col = e.loop_edge ? "var(--magenta)" : "var(--dim2)";
                return <g key={e.id}><path d={`M ${x1} ${y1} C ${x1} ${y1+40}, ${x2} ${y2-40}, ${x2} ${y2}`} fill="none" stroke={col} strokeWidth={e.loop_edge ? 2 : 1.2} strokeDasharray={e.loop_edge ? "6 4" : undefined} /><text x={(x1+x2)/2} y={(y1+y2)/2 - 6} textAnchor="middle" fontSize={9} fill={col}>{e.loop_edge ? "⟲ loop" : ""}</text></g>;
              })}
            </svg>
            {/* plain TUI terminals — shown when no workflow selected; workflow loop uses TUIs when a workflow is selected */}
            {!wf && panes.map((p) => (
              <TuiWindow key={p.id} pane={{ id: p.id, dir: p.dir }} pos={p.pos} selected={selectedTui === p.id} role={tuiRoles.get(p.id)} connecting={connectingFrom === p.id} onStartConnect={(id) => setConnectingFrom(id)} onSelect={(id) => {
                if (connectingFrom && connectingFrom !== id) {
                  const exists = tuiEdges.some((ee) => ee.from === connectingFrom && ee.to === id);
                  if (!exists) {
                    const edge: api.WorkflowEdge = { id: `te-${Date.now().toString(36)}`, from: connectingFrom, to: id, from_port: "out", to_port: "in", condition: null, loop_edge: false };
                    setTuiEdges((ee) => [...ee, edge]);
                  }
                  setConnectingFrom(null);
                }
                setSelectedTui(id); setSelectedNode(null);
              }} onContextMenu={(e, id) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY, nodeId: id }); setSelectedTui(id); setSelectedNode(null); }} onPos={(pos) => setPanes((arr) => arr.map((x) => x.id === p.id ? { ...x, pos } : x))} onDismiss={(id) => {
                void api.tuiStop(id); setPanes((arr) => arr.filter((x) => x.id !== id)); setTuiRoles((m) => { const n = new Map(m); n.delete(id); return n; }); setTuiEdges((ee) => ee.filter((e) => e.from !== id && e.to !== id)); if (selectedTui === id) setSelectedTui(null); if (connectingFrom === id) setConnectingFrom(null);
              }} />
            ))}
            {!wf && connectingFrom && <div style={{ position: "absolute", top: 8, left: "50%", transform: "translateX(-50%)", background: "var(--magenta)", color: "#000", fontSize: 10, padding: "4px 10px", borderRadius: 4, pointerEvents: "none" }}>connecting from {tuiRoles.get(connectingFrom) || connectingFrom.slice(0,8)} — click target terminal to wire{panes.length >= 2 ? " · Esc to cancel" : ""}</div>}
            {!wf && panes.length === 0 && (
              <div style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", pointerEvents: "none" }}>
                <div className="dim" style={{ fontSize: 11, textAlign: "center" }}>
                  birds-eye terminals — click <b>+ TERMINAL</b> to spawn plain opencode<br />
                  normal accept/deny inside the terminal · right-click for extra controls<br />
                  pick a cloud model inside opencode (<code>/model</code>) — no app support needed
                </div>
              </div>
            )}
            {wf && (
              <svg style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", pointerEvents: "none" }}>
                {wf.edges.map((e) => {
                  const a = wf.nodes.find((x) => x.id === e.from);
                  const b = wf.nodes.find((x) => x.id === e.to);
                  if (!a || !b) return null;
                  const x1 = a.pos.x + 310, y1 = a.pos.y + 190, x2 = b.pos.x + 310, y2 = b.pos.y + 190;
                  const col = e.loop_edge ? "var(--magenta)" : e.condition ? "var(--warn)" : "var(--dim2)";
                  const label = e.loop_edge ? "⟲ loop" : e.condition ? `? ${e.condition}` : "";
                  return <g key={e.id}><path d={`M ${x1} ${y1} C ${x1} ${y1+40}, ${x2} ${y2-40}, ${x2} ${y2}`} fill="none" stroke={col} strokeWidth={e.loop_edge ? 2 : 1.2} strokeDasharray={e.loop_edge ? "6 4" : undefined} /><text x={(x1+x2)/2} y={(y1+y2)/2 - 6} textAnchor="middle" fontSize={9} fill={col}>{label}</text></g>;
                })}
              </svg>
            )}
            {/* workflow nodes as large plain TUIs — auto-spawned, no button needed */}
            {wf && wf.nodes.map((n) => {
              const sel = selectedNode === n.id;
              const paneId = wfTuiMap.get(n.id);
              const backing = paneId ? panes.find((pp) => pp.id === paneId) : undefined;
              if (backing) {
                return (
                  <TuiWindow key={n.id} pane={{ id: backing.id, dir: backing.dir }} pos={n.pos} selected={sel} role={n.role_id} connecting={connectingFrom === n.id} onStartConnect={(id) => setConnectingFrom(n.id)} onSelect={() => {
                    if (connectingFrom && connectingFrom !== n.id) {
                      const exists = wf.edges.some((ee) => ee.from === connectingFrom && ee.to === n.id);
                      if (!exists) {
                        const edge: api.WorkflowEdge = { id: `we-${Date.now().toString(36)}`, from: connectingFrom, to: n.id, from_port: "out", to_port: "in", condition: null, loop_edge: false };
                        const upd = { ...wf, edges: [...wf.edges, edge] };
                        void api.workflowSave(JSON.stringify(upd)).then(() => loadWorkflows());
                      }
                      setConnectingFrom(null);
                    }
                    setSelectedNode(n.id); setSelectedTui(null);
                  }} onContextMenu={(e) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY, nodeId: n.id }); setSelectedNode(n.id); }} onPos={(pos) => {
                    const upd = { ...wf, nodes: wf.nodes.map((x) => x.id === n.id ? { ...x, pos } : x) };
                    void api.workflowSave(JSON.stringify(upd)).then(() => loadWorkflows());
                    setPanes((arr) => arr.map((pp) => pp.id === paneId ? { ...pp, pos } : pp));
                  }} onDismiss={() => {
                    const upd = { ...wf, nodes: wf.nodes.filter((x) => x.id !== n.id), edges: wf.edges.filter((e) => e.from !== n.id && e.to !== n.id) };
                    void api.workflowSave(JSON.stringify(upd)).then(() => loadWorkflows());
                    if (paneId) { void api.tuiStop(paneId); setPanes((a) => a.filter((p) => p.id !== paneId)); setWfTuiMap((m) => { const nn = new Map(m); nn.delete(n.id); return nn; }); }
                    spawnedWfNodesRef.current.delete(n.id);
                  }} />
                );
              }
              return (
                <div key={n.id} onClick={() => { setSelectedNode(n.id); setSelectedTui(null); setCtxMenu(null); }} onContextMenu={(e) => { e.preventDefault(); setCtxMenu({ x: e.clientX, y: e.clientY, nodeId: n.id }); setSelectedNode(n.id); }} style={{ position: "absolute", left: n.pos.x, top: n.pos.y, width: 620, height: 380, display: "flex", flexDirection: "column", background: "#0b0b0b", border: `1px solid ${sel ? "var(--magenta)" : "var(--line)"}`, boxShadow: sel ? "0 0 0 2px rgba(210,153,34,0.25)" : "none", overflow: "hidden", cursor: "grab" }}>
                  <div style={{ height: 18, flex: "none", background: sel ? "rgba(210,153,34,0.18)" : "rgba(255,255,255,0.03)", display: "flex", alignItems: "center", gap: 6, padding: "0 6px", fontSize: 9, color: "var(--dim2)" }}>
                    <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>◉ {n.role_id} — terminal not loaded</span>
                    <span onPointerDown={(e) => { e.stopPropagation(); setConnectingFrom(n.id); }} title="wire to another workflow TUI" style={{ width: 10, height: 10, borderRadius: "50%", background: connectingFrom === n.id ? "var(--magenta)" : "var(--line)", border: "1px solid var(--line-bright)", cursor: "crosshair", flex: "none" }} />
                  </div>
                  <div style={{ flex: 1, display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center", color: "var(--dim2)", fontSize: 11, padding: 16, textAlign: "center", gap: 10 }}>
                    <div>Stock opencode terminal for <b style={{ color: "var(--text)" }}>{n.role_id}</b> — not loaded.</div>
                    <button className="action" style={{ fontSize: 11, padding: "6px 12px" }} onClick={async (e) => {
                      e.stopPropagation();
                      try {
                        const paneId = await api.tuiSpawn("/home/deviant/Projects/cyberdeck", 90, 28);
                        setWfTuiMap((m) => { const nn = new Map(m); nn.set(n.id, paneId); return nn; });
                        setPanes((pp) => [...pp, { id: paneId, dir: "/home/deviant/Projects/cyberdeck", pos: n.pos }]);
                        setTuiRoles((mm) => { const nn2 = new Map(mm); nn2.set(paneId, n.role_id); return nn2; });
                        spawnedWfNodesRef.current.add(n.id);
                      } catch (err) { setTuiErr(String(err)); }
                    }}>LOAD TUI</button>
                    <div style={{ fontSize: 9 }}>Pick cloud model inside via <code>/model</code> once loaded. Auto-load failed — click to retry.</div>
                  </div>
                  <div className="mono dim" style={{ fontSize: 9, padding: "4px 8px", borderTop: "1px solid var(--line)", background: "var(--panel-2)" }}>{n.binding.model_ref ? `${n.binding.model_ref} @ ${n.binding.engine || "auto"}` : "no model — picks inside TUI"}</div>
                </div>
              );
            })}
          </div>
          {ctxMenu && (
            <div style={{ position: "fixed", left: ctxMenu.x, top: ctxMenu.y, background: "var(--panel)", border: "1px solid var(--line-bright)", borderRadius: 6, boxShadow: "0 8px 24px rgba(0,0,0,0.5)", zIndex: 99, fontSize: 11, minWidth: 160 }} onClick={() => setCtxMenu(null)}>
              {panes.find((p) => p.id === ctxMenu.nodeId) ? (
                <>
                  <div style={{ padding: "6px 10px", cursor: "pointer" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); void (async () => { const id2 = await api.tuiSpawn("/home/deviant/Projects/cyberdeck", 90, 28); const pp = panes.find((pp) => pp.id === id); const pos = pp ? { x: pp.pos.x + 24, y: pp.pos.y + 24 } : { x: 24, y: 24 }; setPanes((a) => [...a, { id: id2, dir: "/home/deviant/Projects/cyberdeck", pos }]); setSelectedTui(id2); })(); void id; }}>Duplicate terminal</div>
                  <div style={{ padding: "6px 10px", cursor: "pointer", color: "var(--oom)" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); void api.tuiStop(id); setPanes((a) => a.filter((x) => x.id !== id)); if (selectedTui === id) setSelectedTui(null); }}>Close terminal</div>
                  <div style={{ padding: "6px 10px", cursor: "pointer" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); setSelectedTui(id); setSelectedNode(null); }}>Show session panel</div>
                </>
              ) : (
                <>
                  <div style={{ padding: "6px 10px", cursor: "pointer" }} onClick={() => { setCtxMenu(null); const n = wf?.nodes.find((x) => x.id === ctxMenu.nodeId); if (n) { const copy = { ...n, id: `${n.id}-copy-${Date.now().toString(36)}`, role_id: `${n.role_id}_copy`, pos: { x: n.pos.x + 40, y: n.pos.y + 40 } }; const upd = { ...wf!, nodes: [...wf!.nodes, copy] }; void api.workflowSave(JSON.stringify(upd)).then(() => loadWorkflows()).then(() => setSelectedNode(copy.id)); } }}>Duplicate</div>
                  <div style={{ padding: "6px 10px", cursor: "pointer", color: "var(--oom)" }} onClick={() => { const id = ctxMenu.nodeId; setCtxMenu(null); const upd = { ...wf!, nodes: wf!.nodes.filter((nn) => nn.id !== id), edges: wf!.edges.filter((e) => e.from !== id && e.to !== id) }; void api.workflowSave(JSON.stringify(upd)).then(() => { if (selectedNode === id) setSelectedNode(null); void loadWorkflows(); }); void id; }}>Delete</div>
                </>
              )}
            </div>
          )}
          {wfMsg && <div style={{ color: "var(--warn)", marginTop: 8, fontSize: 10 }}>{wfMsg}</div>}
          {harnessErr && <div style={{ background: "rgba(248,81,73,0.1)", border: "1px solid rgba(248,81,73,0.3)", color: "var(--oom)", padding: "6px 10px", fontSize: 11, marginTop: 8 }}>harness error: {harnessErr}</div>}
          {tuiErr && <div style={{ background: "rgba(248,81,73,0.1)", border: "1px solid rgba(248,81,73,0.3)", color: "var(--oom)", padding: "6px 10px", fontSize: 11, marginTop: 8 }}>{tuiErr}</div>}
        </div>

        {/* right drawer: TUI session panel (click terminal) OR node inspector */}
        {(drawerOpen && editing) ? (
          <div style={{ width: 380, flex: "none", overflow: "hidden", display: "flex", flexDirection: "column", borderLeft: "1px solid var(--line)", background: "var(--bg)" }}>
            <div className="row" style={{ justifyContent: "space-between", alignItems: "center", padding: "6px 8px", borderBottom: "1px solid var(--line)" }}>
              <span style={{ fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>LOADOUT — {editing.name || "new"}</span>
              <button className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} onClick={() => { setDrawerOpen(false); setEditing(null); }}>✕ close</button>
            </div>
            <div style={{ flex: 1, overflow: "auto" }}><LoadoutEditor initial={editing} modelPaths={modelPaths} onClose={() => { setDrawerOpen(false); setEditing(null); }} onSaved={() => { setDrawerOpen(false); setEditing(null); onChanged(); }} /></div>
          </div>
        ) : selectedTui ? (
          (() => {
            const p = panes.find((x) => x.id === selectedTui);
            if (!p) return null;
            const sess = sessions.find((s) => s.id === selectedTui);
            const role = tuiRoles.get(p.id) || "";
            const edges = tuiEdges.filter((e) => e.from === p.id || e.to === p.id);
            return (
              <div style={{ width: 360, flex: "none", overflow: "auto", display: "flex", flexDirection: "column", borderLeft: "1px solid var(--line)", background: "var(--bg)", padding: 10, gap: 10 }}>
                <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
                  <span style={{ fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>TERMINAL — {p.id.slice(0, 8)}</span>
                  <button className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} onClick={() => setSelectedTui(null)}>✕</button>
                </div>
                <label style={{ fontSize: 11, color: "var(--muted)" }}>role — click badge or type to assign (for loops)
                  <input list={`roles-${p.id}`} value={role} onChange={(e) => setTuiRoles((m) => { const n = new Map(m); const v = e.target.value.trim(); if (v) n.set(p.id, v); else n.delete(p.id); return n; })} placeholder="primary-developer — or type custom" style={{ width: "100%", background: "var(--panel)", border: "1px solid var(--line)", color: "var(--text)", padding: "4px 6px", fontSize: 11, marginTop: 4 }} />
                  <datalist id={`roles-${p.id}`}><option value="primary-developer" /><option value="architecture-reviewer" /><option value="human" /></datalist>
                </label>
                {role === "human" && <div className="dim" style={{ fontSize: 9, color: "var(--magenta)" }}>human gate — this terminal pauses for your approval after the loop is satisfied</div>}
                <div className="mono dim" style={{ fontSize: 10 }}>dir: {p.dir}</div>
                <div className="mono dim" style={{ fontSize: 10 }}>model: inside terminal via <code>/model</code> — cloud without app support</div>
                <div style={{ borderTop: "1px solid var(--line)", paddingTop: 8 }}>
                  <div style={{ fontSize: 11, fontWeight: 600, color: "var(--muted)", marginBottom: 4 }}>CONNECTIONS — {edges.length ? `${edges.length} edge${edges.length>1?"s":""}` : "no connections"} {panes.length > 1 && <span className="dim">· click ● on another terminal to connect (loopable)</span>}</div>
                  {edges.length === 0 ? <div className="dim" style={{ fontSize: 10 }}>give this terminal a role, then click its ● handle and pick a target terminal to wire. Use <code>loop</code> for developer→reviewer→developer cycle, and a final edge to a <code>human</code> terminal for approval.</div> : edges.map((e) => {
                    const other = e.from === p.id ? e.to : e.from;
                    const dir = e.from === p.id ? "→" : "←";
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
                  <button className="ghost" style={{ fontSize: 11, flex: 1 }} onClick={() => setConnectingFrom(p.id)} title="wire this terminal to another — loopable edge">CONNECT</button>
                  <button className="ghost" style={{ fontSize: 11, flex: 1 }} disabled={tuiEdges.length === 0} onClick={async () => {
                    nodeOutputsRef.current.clear();
                    const nodes: api.WorkflowNode[] = panes.filter((pp) => tuiRoles.get(pp.id)).map((pp) => ({ id: pp.id, role_id: tuiRoles.get(pp.id)!, binding: { role_id: tuiRoles.get(pp.id)!, model_ref: "", engine: null, overrides_json: "{}", active: true }, kind: (tuiRoles.get(pp.id) === "human" ? "Human" : "Agentic") as api.NodeKind, pos: pp.pos, exec: { timeout_s: 300, max_tokens: 4096, max_retries: 1 } }));
                    if (nodes.length < 2) { setWfMsg("need at least 2 terminals with roles to run a loop"); return; }
                    const taskVal = kickoffTask.trim() || prompt.trim() || "";
                    const wfDoc: api.Workflow = { id: `tui-loop-${Date.now().toString(36)}`, name: `TUI Loop ${new Date().toLocaleTimeString()}`, description: "loop from plain terminals — roles + edges", version: 1, nodes, edges: tuiEdges.filter((e) => nodes.some((n) => n.id === e.from) && nodes.some((n) => n.id === e.to)), exec_settings: { max_parallel: 1, global_retries: 0, budget_tokens: 0, budget_wall_s: 0, max_iterations: 6 }, template: false, inputs: taskVal ? { task: taskVal } : {} };
                    try { await api.workflowSave(JSON.stringify(wfDoc)); await api.workflowRun(wfDoc.id, "agentic", dir || "/home/deviant/Projects/cyberdeck", null, taskVal || null); setWfMsg(`TUI loop '${wfDoc.id}' queued — ${nodes.length}n ${wfDoc.edges.length}e${taskVal ? ` — task: ${taskVal.slice(0,40)}` : ""}`); } catch (err) { setWfMsg(String(err)); }
                  }}>▶ RUN LOOP</button>
                </div>
                {(() => {
                  const hasHuman = [...tuiRoles.values()].includes("human");
                  const hasLoop = tuiEdges.some((e) => e.loop_edge);
                  if (!hasHuman || !hasLoop) return <div className="dim" style={{ fontSize: 9, borderTop: "1px solid var(--line)", paddingTop: 6 }}>Tip: assign one terminal <code>human</code>, wire <code>developer → reviewer (loop)</code> with condition <code>contains:CHANGES</code> and <code>reviewer → human</code> with <code>contains:APPROVED</code> — loop runs until reviewer is satisfied, then pauses for your approval.</div>;
                  return null;
                })()}
                <div className="dim" style={{ fontSize: 10, borderTop: "1px solid var(--line)", paddingTop: 8 }}>Each TUI runs stock opencode — accept/deny inside the terminal. Roles + loop edges are extra when clicked. Cloud model via <code>/model</code> in the TUI, no app wiring needed.</div>
                <div className="row" style={{ gap: 6 }}>
                  <button className="ghost" style={{ fontSize: 11, color: "var(--oom)", flex: 1 }} onClick={() => { void api.tuiStop(p.id); setPanes((a) => a.filter((x) => x.id !== p.id)); setSelectedTui(null); }}>CLOSE TERMINAL</button>
                  <button className="ghost" style={{ fontSize: 11, flex: 1 }} onClick={() => setSelectedTui(null)}>DISMISS</button>
                </div>
                {sess && (
                  <div style={{ borderTop: "1px solid var(--line)", paddingTop: 8 }}>
                    <div style={{ fontSize: 11, fontWeight: 600, color: "var(--muted)", marginBottom: 4 }}>AGENT LOG</div>
                    <div className="term" style={{ maxHeight: 120, overflow: "auto", fontSize: 10 }}>{sess.log.map((l,i)=><div key={i}>{l}</div>)}</div>
                  </div>
                )}
              </div>
            );
          })()
        ) : selectedNode && wf ? (
          (() => {
            const n = wf.nodes.find((x) => x.id === selectedNode);
            if (!n) return null;
            return (
              <div style={{ width: 360, flex: "none", overflow: "auto", display: "flex", flexDirection: "column", borderLeft: "1px solid var(--line)", background: "var(--bg)", padding: 10, gap: 10 }}>
                <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
                  <span style={{ fontSize: 11, fontWeight: 700, color: "var(--muted)" }}>NODE — {n.id}</span>
                  <button className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} onClick={() => setSelectedNode(null)}>✕</button>
                </div>
                <div className="mono" style={{ fontSize: 11, color: "var(--cyan)" }}>{n.role_id}</div>
                <div className="mono dim" style={{ fontSize: 10 }}>{n.binding.model_ref || "(no model)"} {n.binding.engine ? `@ ${n.binding.engine}` : ""}</div>
                <div className="row" style={{ gap: 6, marginTop: 4 }}>
                  <button className="action" style={{ fontSize: 11, flex: 1 }} onClick={async () => { await api.workflowSave(JSON.stringify(wf)); setWfMsg(`saved ${wf.id}`); await loadWorkflows(); }}>SAVE</button>
                  <button className="ghost" style={{ fontSize: 11, color: "var(--oom)" }} onClick={async () => { const upd = { ...wf, nodes: wf.nodes.filter((nn) => nn.id !== n.id), edges: wf.edges.filter((e) => e.from !== n.id && e.to !== n.id) }; await api.workflowSave(JSON.stringify(upd)); setSelectedNode(null); await loadWorkflows(); }}>DELETE</button>
                </div>
              </div>
            );
          })()
        ) : null}
      </div>

      {/* footer benches hidden in plain-terminal mode, shown when workflows on */}
      {showWorkflows && (
        <div className="card" style={{ marginTop: 8, fontSize: 11 }}>
          <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--muted)", margin: 0, marginBottom: 6 }}>WHOLE-LOOP BENCH <span className="dim">· loop tok/s</span></h3>
          {loopBench ? <div className="row" style={{ gap: 12 }}><span className="mono">runs <b>{loopBench.runs}</b></span><span className="mono" style={{ color: "var(--pass)" }}>best {loopBench.best_tps.toFixed(1)}</span><span className="mono dim">avg {loopBench.avg_tps.toFixed(1)}</span></div> : <div className="dim">no loop runs yet</div>}
        </div>
      )}
      {showWorkflows && (
        <div className="card" style={{ marginTop: 8, fontSize: 11 }}>
          <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--muted)", margin: 0, marginBottom: 6 }}>PER-ROLE BENCH</h3>
          {bench.length === 0 ? <div className="dim">no per-role bench yet</div> : <table><thead><tr><th>ROLE</th><th>MODEL</th><th>BEST</th><th>AVG</th></tr></thead><tbody>{bench.map((b) => <tr key={`${b.role_id}:${b.model}:${b.engine}`}><td className="mono" style={{ color: "var(--cyan)" }}>{b.role_id}</td><td className="mono">{b.model}</td><td className="mono" style={{ color: "var(--pass)" }}>{b.best_tps.toFixed(1)}</td><td className="mono dim">{b.avg_tps.toFixed(1)}</td></tr>)}</tbody></table>}
        </div>
      )}

      {/* bottom bar — single loadout/model picker like agentic apps; also spawns via + TERMINAL above */}
      <div style={{ padding: "8px 0 6px", position: "sticky", bottom: 0, background: "linear-gradient(180deg, transparent, var(--bg) 18%)" }}>
        <div style={{ display: "flex", gap: 6, alignItems: "center", marginBottom: 6, flexWrap: "wrap" }}>
          <select value={loadout} onChange={(e) => setLoadout(e.target.value)} style={{ background: "var(--panel)", border: "1px solid var(--line)", color: "var(--text)", padding: "6px 10px", fontSize: 11, minWidth: 150 }}>
            <option value="">loadout — none</option>
            {profiles.map((p) => <option key={p.name} value={p.name}>{p.name} · {p.engine}</option>)}
          </select>
          <button className="ghost" style={{ fontSize: 9, padding: "4px 6px" }} onClick={() => { if (loadout) void edit(loadout); else { setEditing(defaultProfile()); setDrawerOpen(true); } }}>{loadout ? "⚙" : "+ loadout"}</button>
          <select value={harnessModel} onChange={(e) => { setHarnessModel(e.target.value); if (e.target.value) setCustomModel(""); }} style={{ background: "var(--panel)", border: "1px solid var(--line)", color: "var(--text)", padding: "6px 10px", fontSize: 11, minWidth: 180 }}>
            <option value="">model — {active ? "loadout default" : "auto (pick inside TUI)"}</option>
            {models.map((m) => <option key={m.path} value={m.path}>{m.name} {isLocalModel(m.path) ? "🔵" : "🟣"}</option>)}
            <option value="__custom">— custom (openrouter/anthropic/ollama) —</option>
          </select>
          {harnessModel === "__custom" && <input value={customModel} onChange={(e) => setCustomModel(e.target.value)} placeholder="openrouter/claude-3.5" style={{ background: "var(--panel)", border: "1px solid var(--magenta)", color: "var(--text)", padding: "6px 10px", fontSize: 11, minWidth: 180 }} />}
          <span className="mono dim" style={{ fontSize: 10 }}>ctx {ctx.toLocaleString()}</span>
          <button className="ghost" style={{ fontSize: 9, padding: "4px 8px" }} onClick={() => setShowAdvanced((v) => !v)}>{showAdvanced ? "hide" : "+ controls"}</button>
          <span className="dim" style={{ fontSize: 10, marginLeft: "auto" }}>cloud model → pick inside terminal via <code>/model</code></span>
        </div>
        {showAdvanced && (
          <div style={{ display: "flex", gap: 12, alignItems: "center", padding: "6px 10px", marginBottom: 6, background: "var(--panel)", border: "1px solid var(--line)", fontSize: 11 }}>
            <label className="row" style={{ gap: 6 }}><input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> auto-approve</label>
            <label className="row" style={{ gap: 6 }}>dir <input value={dir} onChange={(e) => setDir(e.target.value)} style={{ width: 180, background: "var(--bg)", border: "1px solid var(--line)", color: "var(--text)", padding: "2px 6px", fontSize: 11 }} /></label>
          </div>
        )}
        <div style={{ display: "flex", alignItems: "flex-end", gap: 8, background: "var(--panel-2)", border: "1px solid var(--line-bright)", padding: "8px 10px", boxShadow: "0 6px 24px rgba(0,0,0,0.4)" }}>
          <textarea ref={inputRef} value={prompt} onChange={(e) => setPrompt(e.target.value)} onKeyDown={onKey} placeholder="Message the deck… (also spawns via + TERMINAL for plug-and-play TUI)" rows={1} style={{ flex: 1, background: "transparent", border: "none", color: "var(--text)", fontFamily: "inherit", fontSize: 14, resize: "none", outline: "none", minHeight: 24 }} />
          <button className="action" onClick={runAgent} disabled={!prompt.trim() || pending} style={{ padding: "8px 14px", minWidth: 54 }}>↑</button>
        </div>
      </div>
      {humanGate && (
        <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.7)", display: "flex", alignItems: "center", justifyContent: "center", zIndex: 200 }} onClick={() => setHumanGate(null)}>
          <div onClick={(e) => e.stopPropagation()} style={{ background: "var(--panel)", border: "1px solid var(--line-bright)", width: 560, maxHeight: "80vh", overflow: "auto", padding: 16, boxShadow: "0 20px 60px rgba(0,0,0,0.6)" }}>
            <h3 style={{ margin: 0, marginBottom: 8, fontSize: 13, color: "var(--text)" }}>Human approval — code loop finished</h3>
            <div className="dim" style={{ fontSize: 11, marginBottom: 8 }}>Developer → Reviewer loop ran until reviewer was satisfied (condition <code>contains:APPROVED</code>). Now presenting to you for approval. This is the human-in-the-loop gate.</div>
            <pre style={{ background: "var(--panel-2)", border: "1px solid var(--line)", padding: 10, fontSize: 11, whiteSpace: "pre-wrap", maxHeight: 320, overflow: "auto" }}>{humanGate.code}</pre>
            <div className="row" style={{ gap: 8, marginTop: 12 }}>
              <button className="action" style={{ flex: 1 }} onClick={() => setHumanGate(null)}>✓ APPROVE & PRESENT</button>
              <button className="ghost" style={{ flex: 1, color: "var(--warn)", borderColor: "var(--warn)" }} onClick={() => {
                setHumanGate(null);
                // request changes → re-queue the same TUI loop (reviewer will loop back to developer)
                const t = tuiEdges.find((e) => e.loop_edge);
                if (t) setWfMsg("requested changes — re-running loop (reviewer will loop back to developer)");
                // re-run last TUI loop if exists
                const lastLoop = workflows.find((w) => w.id.startsWith("tui-loop-"));
                if (lastLoop) void api.workflowRun(lastLoop.id, "agentic", null);
              }}>↺ REQUEST CHANGES (loop back)</button>
            </div>
            <div className="dim" style={{ fontSize: 10, marginTop: 8, textAlign: "center" }}>Wire: <code>developer → reviewer (loop, condition contains:CHANGES)</code> + <code>reviewer → human (condition contains:APPROVED)</code> — human gate pauses here.</div>
          </div>
        </div>
      )}
    </div>
  );
}
