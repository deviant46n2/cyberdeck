import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import * as br from "../lib/br";
import { latestBySlot, slotKey } from "../lib/portmap";
import EngineBins from "./EngineBins";
import PortMap from "./PortMap";
import { useEngineList } from "../lib/engines";
import TuiWindow from "../components/TuiWindow";

const ENGINE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
  { engine: "Ollama", host: "127.0.0.1", port: 11434 },
];

export default function Hud({
  models,
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
  const [prompt, setPrompt] = useState("");
  const [dir] = useState("/home/deviant/Projects/cyberdeck");
  const [auto, setAuto] = useState(false);
  const [harnessModel, setHarnessModel] = useState("");
  const [customModel, setCustomModel] = useState("");
  const [loadout, setLoadout] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showBins, setShowBins] = useState(false);
  const [showPorts, setShowPorts] = useState(true);
  const [ctx, setCtx] = useState(32768);
  const [bringupEngine, setBringupEngine] = useState<api.EngineId>("llamacpp");
  const localEngines = useEngineList("LocalPath");
  const [status, setStatus] = useState<api.EngineStatus[]>([]);  const [sessions, setSessions] = useState<
    { id: string; prompt: string; log: string[]; running: boolean; model?: string }[]
  >([]);
  // embedded opencode TUIs: one per pane on the canvas
  const [panes, setPanes] = useState<{ id: string; dir: string; pos: { x: number; y: number } }[]>([]);
  // per-card canvas position, kept in a ref so dragging never re-renders stateful cards on every mousemove
  const cardPos = useRef<Map<string, { x: number; y: number }>>(new Map());
  // running count for cascade offsets on new TUIs
  const sessionCountRef = useRef(0);
  const [residents, setResidents] = useState<api.PortMapSlot[]>([]);
  const [benchBySlot, setBenchBySlot] = useState<Map<string, { tps: number; ctx: number; model: string; at: number }>>(
    () => new Map()
  );
  const sessionsRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const active = profiles.find((p) => p.name === loadout) ?? null;

  // Re-probe liveness after residency changes (a stopped slot frees its port).
  const reprobe = useCallback(() => {
    void Promise.all(ENGINE_NODES.map((n) => api.engineStatus(n.engine, n.host, n.port).catch(() => null))).then((r) =>
      setStatus(r.filter(Boolean) as api.EngineStatus[])
    );
  }, []);

  // Fetch residents + bench history for chat header (fit + tok/s per resident)
  const refetchResidents = useCallback(async () => {
    const [slots, hist] = await Promise.all([
      api.portMapStatus("127.0.0.1"),
      api.benchHistory(),
    ]);
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

  // auto-pick a live engine when the current harnessModel points at a down slot
  useEffect(() => {
    const ftUp = status.find((s) => s.engine === "FreeToken")?.up;
    const llUp = status.find((s) => s.engine === "LlamaCpp")?.up;
    const olUp = status.find((s) => s.engine === "Ollama")?.up;
    // if harnessModel is empty, pick a live one
    if (!harnessModel) {
      if (ftUp) setHarnessModel("freetoken/qwen3.6-35b-a3b-nvfp4");
      else if (llUp) setHarnessModel("llamacpp/qwen3.8-27b");
      else if (olUp) setHarnessModel("ollama/qwen3");
      return;
    }
    // if harnessModel points at a down engine, switch
    const currentEngine = harnessModel.split("/")[0]?.toLowerCase();
    const isDown = (currentEngine === "freetoken" && ftUp === false) || (currentEngine === "llamacpp" && llUp === false) || (currentEngine === "ollama" && olUp === false);
    if (isDown) {
      if (ftUp) setHarnessModel("freetoken/qwen3.6-35b-a3b-nvfp4");
      else if (llUp) setHarnessModel("llamacpp/qwen3.8-27b");
      else if (olUp) setHarnessModel("ollama/qwen3");
      else setHarnessErr("No engine is UP — start one in HUD → LOADED MODELS or VAULT → LOAD");
    }
  }, [status, harnessModel]);

  // sessions stream — also merges optimistic bubble with real id
  useEffect(() => {
    const a = listen<{ id: string; prompt: string }>("opencode-started", (e) => {
      console.log("[harness] opencode-started", e.payload);
      setSessions((s) => {
        const pending = s.find((x) => x.id.startsWith("pending-") && x.running);
        if (pending) {
          // carry the pending card's canvas position + model onto the real session id
          const p = cardPos.current.get(pending.id);
          if (p) cardPos.current.set(e.payload.id, p);
          cardPos.current.delete(pending.id);
          return s.map((x) => x.id === pending.id ? { ...x, id: e.payload.id, log: [...x.log, `[deck] session ${e.payload.id} started` ] } : x);
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
      console.log("[harness] done", e.payload);
      setSessions((s) => s.map((x) => (x.id === e.payload.session ? { ...x, running: false } : x)));
    });
    return () => { a.then((f) => f()); b.then((f) => f()); c.then((f) => f()); };
  }, []);

  useEffect(() => {
    if (sessionsRef.current) sessionsRef.current.scrollTop = sessionsRef.current.scrollHeight;
  }, [sessions]);

  const [harnessErr, setHarnessErr] = useState("");
  const [pending, setPending] = useState(false);
  const runAgent = async () => {
    if (!prompt.trim()) return;
    let chosen = (harnessModel === "__custom" ? customModel : harnessModel) || active?.model || "";
    // validate chosen engine is up before spawning — prevents freeze on down :18000
    const ftUp = status.find((s) => s.engine === "FreeToken")?.up;
    const llUp = status.find((s) => s.engine === "LlamaCpp")?.up;
    const olUp = status.find((s) => s.engine === "Ollama")?.up;
    const eng = chosen.split("/")[0]?.toLowerCase();
    if ((eng === "freetoken" && ftUp === false) || (eng === "llamacpp" && llUp === false) || (eng === "ollama" && olUp === false)) {
      setHarnessErr(`Engine ${eng} is DOWN — start it in LOADED MODELS or pick a live model above.`);
      return;
    }
    // fallback to any live engine if chosen is empty
    if (!chosen) {
      if (ftUp) chosen = "freetoken/qwen3.6-35b-a3b-nvfp4";
      else if (llUp) chosen = "llamacpp/qwen3.8-27b";
      else if (olUp) chosen = "ollama/qwen3";
      else { setHarnessErr("No engine is UP — start one in LOADED MODELS or VAULT → LOAD"); return; }
    }
    const snap = prompt;
    setHarnessErr("");
    setPending(true);
    const optimisticId = `pending-${Date.now()}`;
    sessionCountRef.current += 1;
    // cascade new TUIs so they don't stack exactly at (0,0)
    const cascade = (sessionCountRef.current % 5) * 24;
    if (!cardPos.current.has(optimisticId)) cardPos.current.set(optimisticId, { x: cascade, y: cascade });
    setSessions((s) => [...s, { id: optimisticId, prompt: snap.slice(0, 120), log: [`[deck] spawning opencode ${chosen ? `-m ${chosen}` : ""} --dir ${dir} …`, `[deck] waiting for opencode-started event… (model ${chosen} → engine ${eng || 'auto'})`], running: true, model: chosen }]);
    // race opencodeRun against a 15s timeout so a down engine never freezes the HUD
    const withTimeout = <T,>(p: Promise<T>, ms: number, msg: string) => Promise.race([p, new Promise<never>((_, rej) => setTimeout(() => rej(new Error(msg)), ms))]);
    try {
      await withTimeout(api.opencodeRun({ prompt: snap, dir, auto, model: chosen, engine: eng }), 15000, `opencode harness timed out after 15s — is ${eng} on its port UP?`);
      setPrompt("");
      setTimeout(() => inputRef.current?.focus(), 50);
    } catch (e) {
      const msg = String(e);
      setHarnessErr(msg);
      setSessions((s) => s.map((x) => x.id === optimisticId ? { ...x, log: [...x.log, `[harness error] ${msg}`], running: false } : x));
    } finally {
      setPending(false);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      runAgent();
    }
  };

  // spawn a new embedded opencode TUI pane on the canvas
  const [tuiErr, setTuiErr] = useState("");
  const spawnTui = async () => {
    setTuiErr("");
    try {
      const id = await api.tuiSpawn("/home/deviant/Projects/cyberdeck", 80, 24);
      const cascade = (panes.length % 5) * 32 + 8;
      setPanes((p) => [...p, { id, dir: "/home/deviant/Projects/cyberdeck", pos: { x: cascade, y: cascade } }]);
    } catch (e) {
      setTuiErr(`tui spawn failed: ${String(e)}`);
    }
  };

  const hasSessions = sessions.length > 0;

  return (
    <div style={{display:"flex", flexDirection:"column", height:"calc(100vh - 44px)", maxWidth:760, margin:"0 auto", width:"100%"}}>
      {status.find((s)=>s.engine==="FreeToken")?.up===false && status.find((s)=>s.engine==="LlamaCpp")?.up && !harnessModel.includes("freetoken") && (
        <div style={{background:"rgba(255,176,0,0.09)", border:"1px solid rgba(255,176,0,0.22)", color:"var(--warn)", padding:"7px 10px", fontSize:11, textAlign:"center", marginBottom:6}}>
          freetoken :1919 is offline — defaulting harness to <b>llamacpp/qwen3.8-27b</b> (:18000 is live). Pick a model above to override.
        </div>
      )}
      {/* top bar — model/loadout pills + resident summary */}
      <div style={{display:"flex", gap:8, alignItems:"center", padding:"10px 0 6px", flexWrap:"wrap", justifyContent:"center"}}>
        <select
          value={loadout}
          onChange={(e) => setLoadout(e.target.value)}
          title="loadout"
          style={{background:"#0e0e18", border:"1px solid #232336", color:"var(--text)", padding:"6px 10px", fontSize:12, fontFamily:"inherit", minWidth:140}}
        >
          <option value="">loadout — none</option>
          {profiles.map((p) => <option key={p.name} value={p.name}>{p.name} · {p.engine}</option>)}
        </select>
        <select
          value={harnessModel}
          onChange={(e) => { setHarnessModel(e.target.value); if (e.target.value) setCustomModel(""); }}
          title="model — local GGUF/safetensors or ollama blob"
          style={{background:"#0e0e18", border:"1px solid #232336", color:"var(--text)", padding:"6px 10px", fontSize:12, fontFamily:"inherit", minWidth:160}}
        >
          <option value="">model — {active ? "loadout default" : "auto"}</option>
          {models.map((m) => <option key={m.path} value={m.path}>{m.name}</option>)}
          <option value="__custom">— custom (opencode) —</option>
        </select>
        {harnessModel === "__custom" && (
          <input
            value={customModel}
            onChange={(e) => setCustomModel(e.target.value)}
            placeholder="openrouter/anthropic/claude-3.5 or ollama/qwen3"
            style={{background:"#0e0e18", border:"1px solid var(--cyan)", color:"var(--text)", padding:"6px 10px", fontSize:11, fontFamily:"inherit", minWidth:180}}
          />
        )}
        {models.length > 0 && localEngines.length > 0 && (
          <div style={{display:"flex", gap:4, alignItems:"center"}}>
            <select
              value={bringupEngine}
              onChange={(e) => setBringupEngine(e.target.value as api.EngineId)}
              title="engine for one-click load"
              style={{background:"#0e0e18", border:"1px solid #232336", color:"var(--text)", padding:"6px 10px", fontSize:11, fontFamily:"inherit", minWidth:90}}
            >
              {localEngines.map((en) => (
                <option key={en.id} value={en.id}>{en.id === "llamacpp" ? "LCPP" : en.id === "freetoken" ? "FT" : en.id}</option>
              ))}
            </select>
            <button
              className="action"
              style={{fontSize:11, padding:"6px 10px", fontWeight:"bold"}}
              onClick={() => harnessModel && br.startBringup(harnessModel, bringupEngine)}
              disabled={!harnessModel}
              title={`LOAD ${bringupEngine} — derive max-ctx, verify on test port, then go live`}
            >
              LOAD
            </button>
          </div>
        )}
        <button className="ghost" style={{fontSize:11, padding:"6px 10px"}} onClick={()=>setShowAdvanced((v)=>!v)}>
          {showAdvanced ? "− basic" : "+ controls"}
        </button>
        <button className="ghost" style={{fontSize:11, padding:"6px 10px"}} onClick={()=>setShowPorts((v)=>!v)}>
          {showPorts ? "− ports" : "ports"}
        </button>
        <button className="action" style={{fontSize:11, padding:"6px 10px", fontWeight:"bold"}} onClick={()=>spawnTui()} title="spawn a real opencode TUI pane on the canvas">
          + TUI
        </button>
        <button className="ghost" style={{fontSize:11, padding:"6px 10px"}} onClick={()=>setShowBins((v)=>!v)}>
          {showBins ? "− bins" : "bins"}
        </button>
      </div>

      {/* Resident chat header — fit verdict + tok/s per live slot */}
      {residents.some((r) => r.resident && r.profile) && (
        <div style={{display:"flex", gap:8, alignItems:"center", justifyContent:"center", flexWrap:"wrap", paddingBottom:8, fontSize:10}}>
          {residents
            .filter((r) => r.resident && r.profile)
            .map((r) => {
              const b = benchBySlot.get(slotKey(r.engine, r.port));
              const verdictColor = r.fit_verdict === "PASS" ? "var(--pass)" : r.fit_verdict === "WARN" ? "var(--warn)" : r.fit_verdict === "OOM" ? "var(--oom)" : "var(--dim2)";
              return (
                <span key={r.engine} className="mono" style={{display:"flex", alignItems:"center", gap:4, color:"var(--text)"}}>
                  <span style={{width:6, height:6, borderRadius:"50%", background: r.state === "up" ? "var(--pass)" : r.state === "starting" ? "var(--warn)" : "var(--dim2)", boxShadow: r.state === "up" ? "0 0 6px rgba(0,255,157,0.5)" : "none"}} />
                  <span>{r.profile}</span>
                  {r.fit_verdict && <span style={{color: verdictColor}}>{r.fit_verdict}</span>}
                  {b && <span style={{color:"var(--cyan)"}}>{b.tps.toFixed(1)} tok/s</span>}
                </span>
              );
            })}
        </div>
      )}
      {showBins && (
        <EngineBins
          onDone={() => {
            // Re-probe liveness — a bin change shouldn't affect UP state, but
            // keeping the status row honest after the next DEPS apply is cheap.
            void Promise.all(ENGINE_NODES.map((n) => api.engineStatus(n.engine, n.host, n.port).catch(() => null))).then((r) =>
              setStatus(r.filter(Boolean) as api.EngineStatus[])
            );
          }}
        />
      )}
      {showAdvanced && (
        <div style={{display:"flex", gap:12, alignItems:"center", justifyContent:"center", padding:"8px 12px", marginBottom:8, background:"#0c0c16", border:"1px solid #1e1e2e", fontSize:11, flexWrap:"wrap"}}>
          <span className="dim">ctx</span>
          <input type="range" min={2048} max={131072} step={2048} value={ctx} onChange={(e)=>setCtx(parseInt(e.target.value))} style={{width:160}} />
          <span className="mono" style={{fontSize:11}}>{ctx.toLocaleString()}</span>
          <label className="row" style={{gap:6, fontSize:11, color:"var(--muted)"}}>
            <input type="checkbox" checked={auto} onChange={(e)=>setAuto(e.target.checked)} />
            auto-approve
          </label>
          <span className="dim" style={{fontSize:10}}>dir: {dir}</span>
        </div>
      )}

      {/* center area — infinite canvas of moveable TUIs. Each session is a
          free-positioned card; drag via its header updates cardPos (ref, no
          re-render storm) and a lightweight token forces a single paint. */}
      <div ref={sessionsRef} style={{flex:1, overflow:"auto", position:"relative", padding:"8px 4px 16px", minWidth:520}}>
        {!hasSessions ? (
          <div style={{height:"100%", display:"flex", flexDirection:"column", alignItems:"center", justifyContent:"center", textAlign:"center", padding:"40px 20px 20px"}}>
            <div style={{fontSize:13, letterSpacing:4, color:"var(--magenta)", textShadow:"0 0 14px rgba(255,46,196,0.4)", marginBottom:8}}>CYBERDECK</div>
            <div className="dim" style={{fontSize:12, marginBottom:24, letterSpacing:1}}>what should the agent do?</div>
            <div style={{display:"flex", gap:8, flexWrap:"wrap", justifyContent:"center", maxWidth:520}}>
              {[
                "fix the failing fit test",
                "add a CLI flag for KV cache GiB",
                "scaffold a new loadout for qwen3",
                "explain this repo's engine setup",
              ].map((s)=> (
                <button key={s} className="ghost" style={{fontSize:11, padding:"8px 12px", background:"#0e0e18"}} onClick={()=>setPrompt(s)}>{s}</button>
              ))}
            </div>
            {profiles.length > 0 && (
              <div style={{marginTop:22, display:"flex", gap:6, flexWrap:"wrap", justifyContent:"center"}}>
                {profiles.slice(0,5).map((p)=> (
                  <button key={p.name} onClick={()=>setLoadout(p.name)} className={loadout===p.name ? "action" : "ghost"} style={{fontSize:10, padding:"5px 9px"}}>
                    {p.name}
                  </button>
                ))}
                {profiles.length > 5 && <span className="dim" style={{fontSize:10, alignSelf:"center"}}>+{profiles.length-5} in LOADOUTS</span>}
              </div>
            )}
            <div className="dim" style={{fontSize:10, marginTop:18}}>
              {models.length} models · {profiles.length} loadouts · each TUI pins its own model (drag ⠿ to move) · shift+enter for newline
            </div>
          </div>
        ) : (
          sessions.map((s) => {
            const pos = cardPos.current.get(s.id) ?? { x: 0, y: 0 };
            return (
              <div
                key={s.id}
                className="tui-card-wrap"
                style={{ position:"absolute", left:0, top:0, transform:`translate(${pos.x}px, ${pos.y}px)`, width:"100%", maxWidth:560 }}
              >
                {/* prompt + drag handle in one line */}
                <div
                  className="row"
                  style={{ cursor:"grab", gap:6, alignItems:"center", marginBottom:3 }}
                  onPointerDown={(e) => {
                    // drag the whole card by its header/prompt line
                    const startX = e.clientX, startY = e.clientY;
                    const orig = cardPos.current.get(s.id) ?? { x: 0, y: 0 };
                    const move = (ev: PointerEvent) => {
                      cardPos.current.set(s.id, { x: orig.x + (ev.clientX - startX), y: orig.y + (ev.clientY - startY) });
                      const wrap = (ev.target as HTMLElement).closest(".tui-card-wrap") as HTMLElement | null;
                      wrap?.style.setProperty("transform", `translate(${cardPos.current.get(s.id)!.x}px, ${cardPos.current.get(s.id)!.y}px)`);
                    };
                    const up = () => {
                      window.removeEventListener("pointermove", move);
                      window.removeEventListener("pointerup", up);
                    };
                    window.addEventListener("pointermove", move);
                    window.addEventListener("pointerup", up);
                    (e.target as HTMLElement).closest(".tui-card-wrap")?.setPointerCapture?.(e.pointerId);
                  }}
                >
                  <span style={{ color:"var(--dim2)", fontSize:10, userSelect:"none" }}>⠿</span>
                  <div style={{ alignSelf:"flex-end", background:"rgba(255,46,196,0.10)", border:"1px solid rgba(255,46,196,0.18)", padding:"6px 12px", fontSize:13, lineHeight:1.5 }}>
                    {s.prompt}
                  </div>
                </div>
                {/* the TUI body */}
                <div className="tui-card" style={{ background:"#0e0e18", border:"1px solid #1e1e2e", padding:"10px 12px", minHeight:48 }}>
                  <div className="row" style={{ gap:6, marginBottom:6, flexWrap:"wrap" }}>
                    <span className={`dot ${s.running ? "up" : "down"}`} style={{ width:7, height:7 }} />
                    <select
                      value={s.model ?? ""}
                      onChange={(e) => setSessions((a) => a.map((x) => x.id === s.id ? { ...x, model: e.target.value } : x))}
                      title="model for THIS agent"
                      style={{ background:"#060610", border:"1px solid #232336", color:"var(--text)", padding:"2px 6px", fontSize:11, fontFamily:"inherit", maxWidth:210 }}
                    >
                      <option value="">{s.running ? "using active model…" : "pick a model for this agent"}</option>
                      {models.map((m) => <option key={m.path} value={m.path}>{m.name}</option>)}
                    </select>
                    <span className="dim" style={{ fontSize:10 }}>{s.running ? "agent running…" : "done"}</span>
                    {!s.running && <button className="ghost" style={{ marginLeft:"auto", fontSize:10, padding:"3px 7px" }} onClick={()=>setSessions((a)=>a.filter((x)=>x.id!==s.id))}>dismiss</button>}
                    {s.running && <button className="ghost" style={{ marginLeft:"auto", fontSize:10, padding:"3px 7px" }} onClick={()=>api.opencodeStop(s.id)}>stop</button>}
                  </div>
                  <div className="term" style={{ height:180, marginTop:0, background:"#060610", border:"1px solid #141428" }}>
                    {s.log.length===0 ? <span className="dim">starting…</span> : s.log.map((l,i)=><div key={i}>{l}</div>)}
                  </div>
                </div>
              </div>
            );
          })
        )}
        {/* embedded opencode TUI panes — free-floating windows on the canvas */}
        {panes.map((p) => (
          <TuiWindow
            key={p.id}
            pane={{ id: p.id, dir: p.dir }}
            pos={p.pos}
            onPos={(pos) => setPanes((arr) => arr.map((x) => x.id === p.id ? { ...x, pos } : x))}
            onDismiss={(id) => { void api.tuiStop(id); setPanes((arr) => arr.filter((x) => x.id !== id)); }}
          />
        ))}
        {tuiErr && (
          <div style={{ position: "absolute", left: 8, bottom: 8, background:"rgba(255,59,59,0.10)", border:"1px solid rgba(255,59,59,0.25)", color:"var(--oom)", padding:"6px 10px", fontSize:11 }}>
            {tuiErr}
          </div>
        )}
      </div>

      {harnessErr && (
        <div style={{background:"rgba(255,59,59,0.10)", border:"1px solid rgba(255,59,59,0.25)", color:"var(--oom)", padding:"8px 10px", fontSize:11, marginBottom:8}}>
          harness error: {harnessErr}
        </div>
      )}
      {/* bottom input — chatgpt style */}
      <div style={{padding:"12px 0 18px", position:"sticky", bottom:0, background:"linear-gradient(180deg, transparent, var(--bg) 18%)"}}>
        <div style={{
          display:"flex", alignItems:"flex-end", gap:8,
          background:"#10101a", border:"1px solid #232336",
          padding:"8px 10px", boxShadow:"0 6px 24px rgba(0,0,0,0.4), 0 0 0 1px rgba(255,46,196,0.06)",
        }}>
          <textarea
            ref={inputRef}
            value={prompt}
            onChange={(e)=>setPrompt(e.target.value)}
            onKeyDown={onKey}
            placeholder={active ? `Message ${active.name}…` : "Message the deck…"}
            rows={1}
            style={{
              flex:1, background:"transparent", border:"none", color:"var(--text)",
              fontFamily:"inherit", fontSize:14, lineHeight:1.5, resize:"none",
              outline:"none", minHeight:24, maxHeight:140, padding:"6px 2px",
            }}
            onInput={(e)=>{
              const el=e.currentTarget;
              el.style.height="auto";
              el.style.height=Math.min(el.scrollHeight,140)+"px";
            }}
          />
          <button
            className="action"
            onClick={runAgent}
            disabled={!prompt.trim()}
            style={{padding:"8px 14px", minWidth:54}}
            title="send (enter)"
          >
            ↑
          </button>
        </div>
        <div style={{display:"flex", gap:8, justifyContent:"center", marginTop:8, fontSize:10, color:"var(--dim2)", flexWrap:"wrap"}}>
          <span>↵ send · shift+↵ newline</span>
          <span>·</span>
          <span style={{color: loadout ? "var(--magenta)" : undefined}}>{loadout ? `loadout: ${loadout}` : "no loadout"}</span>
          <span>·</span>
          <span style={{color: harnessModel ? "var(--cyan)" : undefined}}>{harnessModel ? harnessModel.split("/").pop()?.slice(0,28) : active?.model ? active.model.split("/").pop()?.slice(0,28) : "auto model"}</span>
          <span>·</span>
          <span className="dim">to test a loadout: open LOADOUTS → EDIT → TEST LOAD</span>
        </div>
      </div>
    </div>
  );
}
