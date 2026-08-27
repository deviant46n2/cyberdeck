import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

const ENGINE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
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
  const [loadout, setLoadout] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [ctx, setCtx] = useState(32768);
  const [status, setStatus] = useState<api.EngineStatus[]>([]);
  const [sessions, setSessions] = useState<
    { id: string; prompt: string; log: string[]; running: boolean }[]
  >([]);
  const sessionsRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const active = profiles.find((p) => p.name === loadout) ?? null;

  useEffect(() => {
    Promise.all(ENGINE_NODES.map((n) => api.engineStatus(n.engine, n.host, n.port).catch(() => null))).then((r) =>
      setStatus(r.filter(Boolean) as api.EngineStatus[])
    );
  }, []);

  // if the default opencode model (freetoken) is offline but llamacpp is up, default the harness to llamacpp
  useEffect(() => {
    if (harnessModel) return;
    const ftUp = status.find((s) => s.engine === "FreeToken")?.up;
    const llUp = status.find((s) => s.engine === "LlamaCpp")?.up;
    if (ftUp === false && llUp === true) setHarnessModel("llamacpp/qwen3.8-27b");
  }, [status, harnessModel]);

  // sessions stream — also merges optimistic bubble with real id
  useEffect(() => {
    const a = listen<{ id: string; prompt: string }>("opencode-started", (e) => {
      console.log("[harness] opencode-started", e.payload);
      setSessions((s) => {
        const pending = s.find((x) => x.id.startsWith("pending-") && x.running);
        if (pending) {
          return s.map((x) => x.id === pending.id ? { ...x, id: e.payload.id, log: [...x.log, `[deck] session ${e.payload.id} started` ] } : x);
        }
        return [...s, { id: e.payload.id, prompt: e.payload.prompt, log: [], running: true }];
      });
    });
    const b = listen<{ session: string; stream: string; text: string }>("opencode-output", (e) => {
      setSessions((s) => {
        const target = s.find((x) => x.id === e.payload.session);
        if (target) return s.map((x) => x.id === e.payload.session ? { ...x, log: [...x.log, e.payload.text] } : x);
        const pending = s.find((x) => x.id.startsWith("pending-") && x.running);
        if (pending) return s.map((x) => x.id === pending.id ? { ...x, id: e.payload.session, log: [...x.log, e.payload.text] } : x);
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
    let chosen = harnessModel || active?.model || "";
    if (!chosen) {
      const ftUp = status.find((s) => s.engine === "FreeToken")?.up;
      const llUp = status.find((s) => s.engine === "LlamaCpp")?.up;
      if (ftUp === false && llUp) chosen = "llamacpp/qwen3.8-27b";
    }
    const snap = prompt;
    setHarnessErr("");
    setPending(true);
    const optimisticId = `pending-${Date.now()}`;
    setSessions((s) => [...s, { id: optimisticId, prompt: snap.slice(0, 120), log: [`[deck] spawning opencode ${chosen ? `-m ${chosen}` : ""} --dir ${dir} …`, "[deck] waiting for opencode-started event…"], running: true }]);
    try {
      await api.opencodeRun({ prompt: snap, dir, auto, model: chosen });
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

  const hasSessions = sessions.length > 0;

  return (
    <div style={{display:"flex", flexDirection:"column", height:"calc(100vh - 44px)", maxWidth:760, margin:"0 auto", width:"100%"}}>
      {status.find((s)=>s.engine==="FreeToken")?.up===false && status.find((s)=>s.engine==="LlamaCpp")?.up && !harnessModel.includes("freetoken") && (
        <div style={{background:"rgba(255,176,0,0.09)", border:"1px solid rgba(255,176,0,0.22)", color:"var(--warn)", padding:"7px 10px", fontSize:11, textAlign:"center", marginBottom:6}}>
          freetoken :1919 is offline — defaulting harness to <b>llamacpp/qwen3.8-27b</b> (:18000 is live). Pick a model above to override.
        </div>
      )}
      {/* top bar — model/loadout pills */}
      <div style={{display:"flex", gap:8, alignItems:"center", padding:"10px 0 14px", flexWrap:"wrap", justifyContent:"center"}}>
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
          onChange={(e) => setHarnessModel(e.target.value)}
          title="model"
          style={{background:"#0e0e18", border:"1px solid #232336", color:"var(--text)", padding:"6px 10px", fontSize:12, fontFamily:"inherit", minWidth:160}}
        >
          <option value="">model — {active ? "loadout default" : "auto"}</option>
          {models.map((m) => <option key={m.path} value={m.path}>{m.name}</option>)}
        </select>
        <button className="ghost" style={{fontSize:11, padding:"6px 10px"}} onClick={()=>setShowAdvanced((v)=>!v)}>
          {showAdvanced ? "− basic" : "+ controls"}
        </button>
        {active && (
          <button
            className="ghost"
            style={{fontSize:11, padding:"6px 10px"}}
            onClick={async()=>{ const r=await api.useProfile(active.name,true); onUnit(r.unit); }}
            title="preview unit"
          >
            preview
          </button>
        )}
      </div>

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

      {/* center area */}
      <div ref={sessionsRef} style={{flex:1, overflowY:"auto", padding:"8px 4px 16px", display:"flex", flexDirection:"column", gap:14}}>
        {!hasSessions ? (
          <div style={{flex:1, display:"flex", flexDirection:"column", alignItems:"center", justifyContent:"center", textAlign:"center", padding:"40px 20px 20px"}}>
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
            <div className="dim" style={{fontSize:10, marginTop:18}}>{models.length} models · {profiles.length} loadouts · shift+enter for newline</div>
          </div>
        ) : (
          sessions.map((s)=> (
            <div key={s.id} style={{display:"flex", flexDirection:"column", gap:8}}>
              <div style={{alignSelf:"flex-end", background:"rgba(255,46,196,0.10)", border:"1px solid rgba(255,46,196,0.18)", padding:"10px 14px", maxWidth:"85%", fontSize:13, lineHeight:1.5}}>
                {s.prompt}
              </div>
              <div style={{alignSelf:"flex-start", background:"#0e0e18", border:"1px solid #1e1e2e", padding:"10px 12px", maxWidth:"92%", width:"100%", minHeight:48}}>
                <div className="row" style={{gap:6, marginBottom:6}}>
                  <span className={`dot ${s.running ? "up" : "down"}`} style={{width:7, height:7}} />
                  <span className="dim" style={{fontSize:10}}>{s.running ? "agent running…" : "done"}</span>
                  {!s.running && <button className="ghost" style={{marginLeft:"auto", fontSize:10, padding:"3px 7px"}} onClick={()=>setSessions((a)=>a.filter((x)=>x.id!==s.id))}>dismiss</button>}
                  {s.running && <button className="ghost" style={{marginLeft:"auto", fontSize:10, padding:"3px 7px"}} onClick={()=>api.opencodeStop(s.id)}>stop</button>}
                </div>
                <div className="term" style={{height:180, marginTop:0, background:"#060610", border:"1px solid #141428"}}>
                  {s.log.length===0 ? <span className="dim">starting…</span> : s.log.map((l,i)=><div key={i}>{l}</div>)}
                </div>
              </div>
            </div>
          ))
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
