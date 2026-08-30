import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import { verdictClass } from "../lib/ui";

function defaultProfile(): api.Profile {
  return {
    name: "",
    engine: "LlamaCpp",
    bin: "/usr/bin/llama-server",
    model: "",
    alias: "model",
    host: "0.0.0.0",
    port: 18000,
    metrics: true,
    ctx_size: 32768,
    ctx_ladder: [49152, 40960, 32768],
    n_gpu_layers: 0,
    ubatch_size: 256,
    flash_attn: true,
    kv_cache_type_k: "q4_0",
    kv_cache_type_v: "q4_0",
    load_mode: "mmap+mlock",
    spec_type: null,
    draft_model: null,
    temperature: 0.7,
    top_p: 0.8,
    top_k: 20,
    parallel: 1,
    reasoning: "on",
    reasoning_format: "deepseek",
    reasoning_effort: "medium",
    reasoning_budget: 4096,
    ft_backend: null,
    ft_moe_cache_size: null,
    mem_max_mb: null,
    mem_swap_max_mb: null,
  };
}

function kvBytes(t: string | null): number {
  switch ((t || "").toLowerCase()) {
    case "fp16":
      return 2.0;
    case "fp32":
      return 4.0;
    case "q8_0":
      return 1.0;
    case "q6_0":
      return 0.75;
    case "q5_0":
    case "q5_1":
      return 0.625;
    default:
      return 0.5;
  }
}

const KV_TYPES = ["q4_0", "q4_1", "q5_0", "q5_1", "q6_0", "q8_0", "fp16", "fp32"];

export default function LoadoutEditor({
  initial,
  modelPaths,
  onClose,
  onSaved,
}: {
  initial: api.Profile;
  modelPaths: string[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const [p, setP] = useState<api.Profile>(initial);
  const [fit, setFit] = useState<api.FitRow | null>(null);
  const [fitErr, setFitErr] = useState("");
  const [unit, setUnit] = useState("");
  const [msg, setMsg] = useState("");

  const [phase, setPhase] = useState("");
  const [log, setLog] = useState<string[]>([]);
  const [result, setResult] = useState<{ verdict: string; summary: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [advanced, setAdvanced] = useState(false);

  const set = <K extends keyof api.Profile>(k: K, v: api.Profile[K]) =>
    setP((prev) => {
      const next = { ...prev, [k]: v };
      if (k === "engine") {
        next.ft_backend = v === "FreeToken" ? "offload" : null;
        if (v === "FreeToken") next.bin = "/usr/local/bin/ft";
      }
      return next;
    });

  const offload = p.engine === "FreeToken" && p.ft_backend === "offload";

  useEffect(() => {
    setResult(null);
    const t = setTimeout(async () => {
      if (!p.model) {
        setFit(null);
        setFitErr("");
        setUnit("");
        return;
      }
      try {
        const f = await api.fit({
          model: p.model,
          ctx: p.ctx_size,
          kv_bytes: kvBytes(p.kv_cache_type_k),
          n_gpu_layers: p.n_gpu_layers,
          kv_layers: null,
          reserve: 1600,
          offload,
        });
        setFit(f);
        setFitErr("");
      } catch (e) {
        setFit(null);
        setFitErr(`estimate unavailable: ${String(e)}`);
      }
      try {
        setUnit(await api.renderProfileUnit(p));
      } catch {
        /* best-effort */
      }
    }, 300);
    return () => clearTimeout(t);
  }, [p, offload]);

  const testRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const ph = listen<{ phase: string }>("test-phase", (e) => setPhase(e.payload.phase));
    const out = listen<{ stream: string; text: string }>("test-output", (e) =>
      setLog((l) => [...l, e.payload.text])
    );
    const done = listen<{ verdict: string; summary: string }>("test-result", (e) => {
      setResult(e.payload);
      setTesting(false);
    });
    return () => {
      ph.then((f) => f());
      out.then((f) => f());
      done.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (testRef.current) testRef.current.scrollTop = testRef.current.scrollHeight;
  }, [log]);

  const save = async () => {
    if (!p.name.trim()) {
      setMsg("name is required");
      return;
    }
    try {
      await api.saveProfile(p);
      setMsg(`saved '${p.name}'`);
      onSaved();
    } catch (e) {
      setMsg(`save failed: ${String(e)}`);
    }
  };

  const runTest = async () => {
    if (!p.model) {
      setMsg("set a model before testing");
      return;
    }
    if (
      !confirm(
        "TEST LOAD will STOP your live " +
          (p.engine === "FreeToken" ? "freetoken" : "llamacpp") +
          " service, launch this loadout in isolation on a test port, then restart the live service. Continue?"
      )
    )
      return;
    setLog([`[deck] TEST LOAD → ${p.engine} on :${api.TEST_PORTS[p.engine]} — stopping live service (3s delay for VRAM free)…`]);
    setResult(null);
    setPhase("starting");
    setTesting(true);
    setMsg("");
    try {
      await api.testLoadout(p, api.TEST_PORTS[p.engine]);
    } catch (e) {
      const m = String(e);
      setLog((l) => [...l, `[deck] invoke failed: ${m}`]);
      setResult({ verdict: "ERROR", summary: m });
      setMsg(m);
      setTesting(false);
    }
  };

  const num = (v: string) => (v === "" ? null : Number(v));
  const isFt = p.engine === "FreeToken";

  return (
    <div className="editor-backdrop" onClick={onClose}>
      <div className="editor" onClick={(e) => e.stopPropagation()}>
        <div className="editor-chrome">
          <span className="dots"><i /><i /><i /></span>
          <span className="title"><b>deck</b> — agent:{initial.name ? ` ${initial.name}` : " new"} — {p.engine === "FreeToken" ? "freetoken" : "llama.cpp"} <span className="dim">[{advanced ? "advanced" : "basic"}]</span></span>
          <span style={{marginLeft:"auto", fontSize:10, color:"var(--dim2)"}}>ESC to close</span>
        </div>

        <div className="tty">
          {/* fake command line */}
          <div style={{color:"var(--muted)", fontSize:11, marginBottom:8}}>
            <span className="tty-prompt">deck@local</span>:<span style={{color:"var(--cyan)"}}>~/agents</span>$ deck agent {initial.name ? "edit" : "init"} <span className="tty-cmd">{p.name ? `"${p.name}"` : ""}</span> --engine {p.engine.toLowerCase()} {p.model ? `--model "${p.model}"` : ""} <span className="cursor" style={{height:10, width:7, verticalAlign:"-1px"}} />
          </div>
          <div className="tty-dim" style={{fontSize:10, marginBottom:10, letterSpacing:"0.5px"}}>
            every flag editable · live fit estimate · <span style={{color:"var(--warn)"}}>TEST LOAD</span> probes for OOM on a test port
          </div>

          {/* engine switch + advanced */}
          <div className="row" style={{gap:8, marginBottom:10}}>
            {(["LlamaCpp", "FreeToken"] as const).map((e) => (
              <button
                key={e}
                className={p.engine === e ? "action" : "ghost"}
                style={{fontSize:11, padding:"5px 10px"}}
                onClick={() => set("engine", e)}
              >
                {p.engine === e ? "● " : "○ "}{e === "LlamaCpp" ? "llama.cpp" : "freetoken"}
              </button>
            ))}
            <span className="dim" style={{fontSize:10, maxWidth:340}}>
              {isFt ? "offload: weights spill to RAM, VRAM holds KV+buffers" : "full VRAM map via n_gpu_layers (0 = all)"}
            </span>
            <button className="ghost" style={{marginLeft:"auto", fontSize:10}} onClick={() => setAdvanced((a) => !a)}>
              {advanced ? "[−] BASIC" : "[+] ADVANCED"}
            </button>
          </div>

          <div className="tty-grid">
            {/* IDENTITY */}
            <div className="tty-block" data-label="IDENTITY">
              <div className="tty-field">
                <div className="tty-label">name</div>
                <input className="tty-input" value={p.name} placeholder="my-qwen3-agent" onChange={(e) => set("name", e.target.value)} />
              </div>
              {advanced && (
                <div className="tty-field">
                  <div className="tty-label">binary <span className="tty-dim">--bin</span></div>
                  <input className="tty-input" value={p.bin} onChange={(e) => set("bin", e.target.value)} />
                </div>
              )}
              <div className="tty-field">
                <div className="tty-label">model <span className="tty-dim">path or HF id</span></div>
                <input
                  className="tty-input"
                  list="model-paths"
                  value={p.model}
                  placeholder="/models/qwen3-8b.gguf"
                  onChange={(e) => set("model", e.target.value)}
                />
                <datalist id="model-paths">
                  {modelPaths.map((m) => (
                    <option key={m} value={m} />
                  ))}
                </datalist>
              </div>
              <div className="row" style={{gap:8}}>
                <div className="tty-field" style={{flex:1}}>
                  <div className="tty-label">alias</div>
                  <input className="tty-input" value={p.alias} onChange={(e) => set("alias", e.target.value)} />
                </div>
                <div className="tty-field" style={{flex:1}}>
                  <div className="tty-label">port</div>
                  <input className="tty-input" type="number" value={p.port} onChange={(e) => set("port", Number(e.target.value))} />
                </div>
              </div>
              <div className="tty-field">
                <div className="tty-label">host</div>
                <input className="tty-input" value={p.host} onChange={(e) => set("host", e.target.value)} />
              </div>
              {advanced && (
                <label className="row" style={{gap:6, fontSize:11, color:"var(--muted)", marginTop:4}}>
                  <input type="checkbox" checked={p.metrics} onChange={(e) => set("metrics", e.target.checked)} />
                  <span>expose <span className="mono" style={{color:"var(--cyan)"}}>/metrics</span> (BENCH tok/s)</span>
                </label>
              )}
            </div>

            {/* CONTEXT */}
            <div className="tty-block" data-label="CONTEXT / OFFLOAD">
              <div className="row" style={{gap:8}}>
                <div className="tty-field" style={{flex:1}}>
                  <div className="tty-label">ctx_size</div>
                  <input className="tty-input" type="number" value={p.ctx_size} onChange={(e) => set("ctx_size", Number(e.target.value))} />
                </div>
                <div className="tty-field" style={{flex:1}}>
                  <div className="tty-label">n_gpu_layers <span className="tty-dim">0=all</span></div>
                  <input className="tty-input" type="number" value={p.n_gpu_layers} onChange={(e) => set("n_gpu_layers", Number(e.target.value))} />
                </div>
              </div>
              {advanced && (
                <div className="tty-field">
                  <div className="tty-label">ctx_ladder <span className="tty-dim">comma sep</span></div>
                  <input
                    className="tty-input"
                    value={p.ctx_ladder.join(",")}
                    onChange={(e) =>
                      set(
                        "ctx_ladder",
                        e.target.value.split(",").map((s) => s.trim()).filter(Boolean).map(Number)
                      )
                    }
                  />
                </div>
              )}
              <div className="row" style={{gap:8}}>
                <div className="tty-field" style={{flex:1}}>
                  <div className="tty-label">kv K</div>
                  <select className="tty-input" value={p.kv_cache_type_k || ""} onChange={(e) => set("kv_cache_type_k", e.target.value || null)}>
                    <option value="">(default)</option>
                    {KV_TYPES.map((t) => (<option key={t} value={t}>{t}</option>))}
                  </select>
                </div>
                <div className="tty-field" style={{flex:1}}>
                  <div className="tty-label">kv V</div>
                  <select className="tty-input" value={p.kv_cache_type_v || ""} onChange={(e) => set("kv_cache_type_v", e.target.value || null)}>
                    <option value="">(default)</option>
                    {KV_TYPES.map((t) => (<option key={t} value={t}>{t}</option>))}
                  </select>
                </div>
              </div>
              {advanced && (
                <div className="tty-field">
                  <div className="tty-label">load_mode</div>
                  <input className="tty-input" value={p.load_mode || ""} placeholder="mmap+mlock" onChange={(e) => set("load_mode", e.target.value || null)} />
                </div>
              )}
              <label className="row" style={{gap:6, fontSize:11, color:"var(--muted)", marginTop:4}}>
                <input type="checkbox" checked={p.flash_attn} onChange={(e) => set("flash_attn", e.target.checked)} />
                <span>flash_attn</span>
              </label>
            </div>

            {/* ENGINE SPECIFIC */}
            <div className="tty-block" data-label={isFt ? "FREETOKEN" : "SPEC / REASON"}>
              {isFt ? (
                <>
                  <div className="tty-field">
                    <div className="tty-label">moe backend</div>
                    <select className="tty-input" value={p.ft_backend || ""} onChange={(e) => set("ft_backend", e.target.value || null)}>
                      <option value="">(default)</option>
                      <option value="offload">offload</option>
                      <option value="flashinfer">flashinfer</option>
                    </select>
                  </div>
                  <div className="tty-field">
                    <div className="tty-label">moe_cache_size</div>
                    <input className="tty-input" type="number" value={p.ft_moe_cache_size ?? ""} placeholder="3000" onChange={(e) => set("ft_moe_cache_size", num(e.target.value) as never)} />
                  </div>
                  <div className="tty-hint">offload spills expert weights to RAM; VRAM keeps KV+buffers.</div>
                </>
              ) : (
                <>
                  <div className="tty-field">
                    <div className="tty-label">spec_type</div>
                    <input className="tty-input" value={p.spec_type || ""} placeholder="mtp" onChange={(e) => set("spec_type", e.target.value || null)} />
                  </div>
                  <div className="tty-field">
                    <div className="tty-label">draft_model</div>
                    <input className="tty-input" value={p.draft_model || ""} placeholder="path or HF id" onChange={(e) => set("draft_model", e.target.value || null)} />
                  </div>
                  {advanced && (
                    <>
                      <div className="tty-field">
                        <div className="tty-label">reasoning</div>
                        <input className="tty-input" value={p.reasoning || ""} onChange={(e) => set("reasoning", e.target.value || null)} />
                      </div>
                      <div className="row" style={{gap:8}}>
                        <div className="tty-field" style={{flex:1}}>
                          <div className="tty-label">reason_format</div>
                          <input className="tty-input" value={p.reasoning_format || ""} onChange={(e) => set("reasoning_format", e.target.value || null)} />
                        </div>
                        <div className="tty-field" style={{flex:1}}>
                          <div className="tty-label">reason_effort</div>
                          <input className="tty-input" value={p.reasoning_effort || ""} onChange={(e) => set("reasoning_effort", e.target.value || null)} />
                        </div>
                      </div>
                      <div className="tty-field">
                        <div className="tty-label">reasoning_budget</div>
                        <input className="tty-input" type="number" value={p.reasoning_budget ?? ""} onChange={(e) => set("reasoning_budget", num(e.target.value) as never)} />
                      </div>
                    </>
                  )}
                </>
              )}
            </div>

            {/* SAMPLING */}
            {advanced ? (
              <div className="tty-block" data-label="SAMPLING / RESOURCES">
                <div className="row" style={{gap:8}}>
                  <div className="tty-field" style={{flex:1}}>
                    <div className="tty-label">temperature</div>
                    <input className="tty-input" type="number" step="0.05" value={p.temperature} onChange={(e) => set("temperature", Number(e.target.value))} />
                  </div>
                  <div className="tty-field" style={{flex:1}}>
                    <div className="tty-label">top_p</div>
                    <input className="tty-input" type="number" step="0.05" value={p.top_p} onChange={(e) => set("top_p", Number(e.target.value))} />
                  </div>
                </div>
                <div className="row" style={{gap:8}}>
                  <div className="tty-field" style={{flex:1}}>
                    <div className="tty-label">top_k</div>
                    <input className="tty-input" type="number" value={p.top_k} onChange={(e) => set("top_k", Number(e.target.value))} />
                  </div>
                  <div className="tty-field" style={{flex:1}}>
                    <div className="tty-label">parallel</div>
                    <input className="tty-input" type="number" value={p.parallel} onChange={(e) => set("parallel", Number(e.target.value))} />
                  </div>
                </div>
                <div className="tty-field">
                  <div className="tty-label">mem_max <span className="tty-dim">MiB · cgroup MemoryMax</span></div>
                  <input className="tty-input" type="number" value={p.mem_max_mb ?? ""} placeholder="—" onChange={(e) => set("mem_max_mb", num(e.target.value) as never)} />
                </div>
                <div className="tty-field">
                  <div className="tty-label">mem_swap_max <span className="tty-dim">MiB</span></div>
                  <input className="tty-input" type="number" value={p.mem_swap_max_mb ?? ""} placeholder="—" onChange={(e) => set("mem_swap_max_mb", num(e.target.value) as never)} />
                </div>
                <div className="tty-field">
                  <div className="tty-label">ubatch_size</div>
                  <input className="tty-input" type="number" value={p.ubatch_size} onChange={(e) => set("ubatch_size", Number(e.target.value))} />
                </div>
              </div>
            ) : (
              <div className="tty-block" data-label="SAMPLING">
                <div className="tty-dim" style={{fontSize:10, marginBottom:8}}>hidden — toggle ADVANCED for temp/top_p/top_k/parallel + cgroup limits</div>
                <div className="row" style={{gap:8}}>
                  <div className="tty-field" style={{flex:1}}>
                    <div className="tty-label">temperature</div>
                    <input className="tty-input" type="number" step="0.05" value={p.temperature} onChange={(e) => set("temperature", Number(e.target.value))} />
                  </div>
                  <div className="tty-field" style={{flex:1}}>
                    <div className="tty-label">top_p</div>
                    <input className="tty-input" type="number" step="0.05" value={p.top_p} onChange={(e) => set("top_p", Number(e.target.value))} />
                  </div>
                </div>
              </div>
            )}

            {/* LIVE FIT */}
            <div className="tty-block" data-label="LIVE FIT">
              <div style={{display:"flex", alignItems:"center", gap:8, marginBottom:6}}>
                <span className="tty-dim" style={{fontSize:10}}>estimate</span>
                {fit && <span className={`badge ${verdictClass(fit.verdict)}`} style={{fontSize:10}}>{fit.verdict}</span>}
                {fit && <span className="mono" style={{fontSize:10, color:"var(--muted)"}}>{fit.model_vram_mb.toLocaleString()} MiB VRAM</span>}
              </div>
              {fitErr && <div className="tty-warn" style={{fontSize:11}}>{fitErr}</div>}
              {!fit && !fitErr && <div className="tty-dim" style={{fontSize:11}}><span className="tty-prompt">$</span> deck fit --model "{p.model || "…"}" … waiting for model</div>}
              {fit && (
                <div className="tty-fit mono" style={{fontSize:11, lineHeight:1.6}}>
                  <div>weights (VRAM) <span style={{float:"right", color:"var(--text)"}}>{fit.weights_mb} MiB</span></div>
                  {fit.weights_ram_mb > 0 && <div>weights (RAM) <span style={{float:"right"}}>{fit.weights_ram_mb} MiB</span></div>}
                  <div>kv_cache <span style={{float:"right"}}>{fit.kv_mb} MiB</span></div>
                  <div>buffers <span style={{float:"right"}}>{fit.buffers_mb} MiB</span></div>
                  <div style={{borderTop:"1px solid var(--line)", margin:"6px 0 4px", paddingTop:4}}>model VRAM <span style={{float:"right", color:"var(--magenta)"}}>{fit.model_vram_mb} MiB</span></div>
                  <div>desktop reserve <span style={{float:"right"}}>{fit.overhead_mb} MiB</span></div>
                  <div>available <span style={{float:"right", color: fit.verdict==="PASS"? "var(--pass)" : fit.verdict==="WARN"?"var(--warn)":"var(--oom)"}}>{fit.available_for_model_mb} MiB</span></div>
                </div>
              )}
            </div>

            {/* TEST */}
            <div className="tty-block" data-label="TEST LOAD">
              <div className="tty-dim" style={{fontSize:10, marginBottom:6}}>launches on test port — live service paused, watches for OOM/crash</div>
              <div className="row" style={{gap:8, marginBottom:8}}>
                <button className="action" onClick={runTest} disabled={testing} style={{fontSize:11}}>
                  {testing ? "● TESTING…" : "▸ TEST LOAD"}
                </button>
                {testing && <button className="ghost" onClick={() => api.testStop()}>STOP</button>}
                <span className="mono" style={{fontSize:10, color: phase==="idle"||!phase? "var(--dim2)" : "var(--cyan)"}}>phase: {phase || "idle"}</span>
              </div>
              <div className="tty-log" ref={testRef}>
                {log.length === 0 ? <span className="tty-dim">$ waiting for test output…</span> : log.map((l, i) => <div key={i}>{l}</div>)}
              </div>
              {result && (
                <div style={{marginTop:8, display:"flex", gap:8, alignItems:"center"}}>
                  <span className={`badge ${verdictClass(result.verdict)}`}>{result.verdict}</span>
                  <span className="dim" style={{fontSize:11}}>{result.summary}</span>
                </div>
              )}
            </div>
          </div>

          {unit && (
            <div style={{marginTop:12}}>
              <div style={{fontSize:10, letterSpacing:"1.5px", color:"var(--muted)", marginBottom:6}}>┌─ RENDERED UNIT ──────────────────────────────────────┐</div>
              <pre className="unit" style={{marginTop:0}}>{unit}</pre>
            </div>
          )}

          <div className="row" style={{gap:8, marginTop:14, borderTop:"1px solid var(--line)", paddingTop:12}}>
            <span className="tty-prompt" style={{fontSize:12}}>deck@local:~$</span>
            <button className="action" onClick={save}>SAVE</button>
            <button className="ghost" onClick={onClose}>CLOSE</button>
            <span className="mono" style={{fontSize:11, color: msg.includes("failed")||msg.includes("required") ? "var(--oom)" : "var(--pass)"}}>{msg && `› ${msg}`}</span>
          </div>
        </div>
      </div>
    </div>
  );
}

export { defaultProfile };
