import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

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

function verdictClass(v: string): string {
  if (v === "PASS") return "pass";
  if (v === "WARN") return "warn";
  return "oom";
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

  // --- test harness state ---
  const [phase, setPhase] = useState("");
  const [log, setLog] = useState<string[]>([]);
  const [result, setResult] = useState<{ verdict: string; summary: string } | null>(null);
  const [testing, setTesting] = useState(false);
  const [advanced, setAdvanced] = useState(false);

  const set = <K extends keyof api.Profile>(k: K, v: api.Profile[K]) =>
    setP((prev) => {
      const next = { ...prev, [k]: v };
      // Engine swap toggles the FreeToken offload relationship.
      if (k === "engine") {
        next.ft_backend = v === "FreeToken" ? "offload" : null;
        if (v === "FreeToken") next.bin = "/usr/local/bin/ft";
      }
      return next;
    });

  const offload = p.engine === "FreeToken" && p.ft_backend === "offload";

  // Live fit + unit preview (debounced).
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
        /* unit preview is best-effort */
      }
    }, 300);
    return () => clearTimeout(t);
  }, [p, offload]);

  // Test event stream.
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
    setLog([]);
    setResult(null);
    setPhase("starting");
    setTesting(true);
    try {
      await api.testLoadout(p, api.TEST_PORTS[p.engine]);
    } catch (e) {
      setResult({ verdict: "ERROR", summary: String(e) });
      setTesting(false);
    }
  };

  const num = (v: string) => (v === "" ? null : Number(v));
  const isFt = p.engine === "FreeToken";

  return (
    <div className="editor-backdrop" onClick={onClose}>
      <div className="editor" onClick={(e) => e.stopPropagation()}>
        <div className="view-title">
          {initial.name ? `EDIT — ${initial.name}` : "NEW LOADOUT"}
        </div>

        {/* Engine toggle */}
        <div className="row" style={{ gap: 10, margin: "8px 0 14px" }}>
          {(["LlamaCpp", "FreeToken"] as const).map((e) => (
            <button
              key={e}
              className={p.engine === e ? "action" : "ghost"}
              onClick={() => set("engine", e)}
            >
              {e === "LlamaCpp" ? "llama.cpp" : "FreeToken"}
            </button>
          ))}
          <span className="dim" style={{ fontSize: 11 }}>
            {isFt
              ? "offload backend spills weights to RAM; VRAM holds KV + buffers"
              : "full weights mapped to VRAM via n_gpu_layers"}
          </span>
          <button
            className="ghost"
            style={{ marginLeft: "auto" }}
            onClick={() => setAdvanced((a) => !a)}
          >
            {advanced ? "HIDE ADVANCED ▴" : "ADVANCED ▾"}
          </button>
        </div>

        <div className="editor-grid">
          {/* Identity */}
          <div className="card">
            <h3>IDENTITY</h3>
            <Field label="name">
              <input value={p.name} onChange={(e) => set("name", e.target.value)} />
            </Field>
            {advanced && (
              <Field label="binary">
                <input value={p.bin} onChange={(e) => set("bin", e.target.value)} />
              </Field>
            )}
            <Field label="model (path or HF id)">
              <input
                list="model-paths"
                value={p.model}
                onChange={(e) => set("model", e.target.value)}
              />
              <datalist id="model-paths">
                {modelPaths.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
            </Field>
            <div className="row" style={{ gap: 10 }}>
              <Field label="alias" half>
                <input value={p.alias} onChange={(e) => set("alias", e.target.value)} />
              </Field>
              <Field label="port" half>
                <input
                  type="number"
                  value={p.port}
                  onChange={(e) => set("port", Number(e.target.value))}
                />
              </Field>
            </div>
            <Field label="host">
              <input value={p.host} onChange={(e) => set("host", e.target.value)} />
            </Field>
            {advanced && (
              <label className="row" style={{ gap: 8, fontSize: 12 }}>
                <input
                  type="checkbox"
                  checked={p.metrics}
                  onChange={(e) => set("metrics", e.target.checked)}
                />
                <span>expose /metrics (for BENCH tok/s)</span>
              </label>
            )}
          </div>

          {/* Context / offload */}
          <div className="card">
            <h3>CONTEXT / OFFLOAD</h3>
            <div className="row" style={{ gap: 10 }}>
              <Field label="ctx size" half>
                <input
                  type="number"
                  value={p.ctx_size}
                  onChange={(e) => set("ctx_size", Number(e.target.value))}
                />
              </Field>
              <Field label="n_gpu_layers (0=all)" half>
                <input
                  type="number"
                  value={p.n_gpu_layers}
                  onChange={(e) => set("n_gpu_layers", Number(e.target.value))}
                />
              </Field>
            </div>
            {advanced && (
              <Field label="ctx ladder (comma sep)">
                <input
                  value={p.ctx_ladder.join(",")}
                  onChange={(e) =>
                    set(
                      "ctx_ladder",
                      e.target.value
                        .split(",")
                        .map((s) => s.trim())
                        .filter(Boolean)
                        .map(Number)
                    )
                  }
                />
              </Field>
            )}
            <div className="row" style={{ gap: 10 }}>
              <Field label="kv cache K" half>
                <select
                  value={p.kv_cache_type_k || ""}
                  onChange={(e) => set("kv_cache_type_k", e.target.value || null)}
                >
                  <option value="">(default)</option>
                  {KV_TYPES.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="kv cache V" half>
                <select
                  value={p.kv_cache_type_v || ""}
                  onChange={(e) => set("kv_cache_type_v", e.target.value || null)}
                >
                  <option value="">(default)</option>
                  {KV_TYPES.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
            {advanced && (
              <Field label="load mode">
                <input
                  value={p.load_mode || ""}
                  placeholder="e.g. mmap+mlock"
                  onChange={(e) => set("load_mode", e.target.value || null)}
                />
              </Field>
            )}
            <label className="row" style={{ gap: 8, fontSize: 12 }}>
              <input
                type="checkbox"
                checked={p.flash_attn}
                onChange={(e) => set("flash_attn", e.target.checked)}
              />
              <span>flash attention</span>
            </label>
          </div>

          {/* Engine-specific */}
          <div className="card">
            <h3>{isFt ? "FREETOKEN" : "SPECULATIVE / REASONING"}</h3>
            {isFt ? (
              <>
                <Field label="moe backend">
                  <select
                    value={p.ft_backend || ""}
                    onChange={(e) => set("ft_backend", e.target.value || null)}
                  >
                    <option value="">(default)</option>
                    <option value="offload">offload</option>
                    <option value="flashinfer">flashinfer</option>
                  </select>
                </Field>
                <Field label="moe cache size">
                  <input
                    type="number"
                    value={p.ft_moe_cache_size ?? ""}
                    placeholder="e.g. 3000"
                    onChange={(e) =>
                      set("ft_moe_cache_size", num(e.target.value) as never)
                    }
                  />
                </Field>
              </>
            ) : (
              <>
                <Field label="spec type">
                  <input
                    value={p.spec_type || ""}
                    placeholder="e.g. mtp"
                    onChange={(e) => set("spec_type", e.target.value || null)}
                  />
                </Field>
                <Field label="draft model">
                  <input
                    value={p.draft_model || ""}
                    placeholder="path or HF id"
                    onChange={(e) => set("draft_model", e.target.value || null)}
                  />
                </Field>
                {advanced && (
                  <>
                    <Field label="reasoning">
                      <input
                        value={p.reasoning || ""}
                        onChange={(e) => set("reasoning", e.target.value || null)}
                      />
                    </Field>
                    <div className="row" style={{ gap: 10 }}>
                      <Field label="reason format" half>
                        <input
                          value={p.reasoning_format || ""}
                          onChange={(e) => set("reasoning_format", e.target.value || null)}
                        />
                      </Field>
                      <Field label="reason effort" half>
                        <input
                          value={p.reasoning_effort || ""}
                          onChange={(e) => set("reasoning_effort", e.target.value || null)}
                        />
                      </Field>
                    </div>
                    <Field label="reasoning budget">
                      <input
                        type="number"
                        value={p.reasoning_budget ?? ""}
                        onChange={(e) =>
                          set("reasoning_budget", num(e.target.value) as never)
                        }
                      />
                    </Field>
                  </>
                )}
              </>
            )}
          </div>

          {/* Sampling + resources */}
          {advanced && (
          <div className="card">
            <h3>SAMPLING / RESOURCES</h3>
            <div className="row" style={{ gap: 10 }}>
              <Field label="temperature" half>
                <input
                  type="number"
                  step="0.05"
                  value={p.temperature}
                  onChange={(e) => set("temperature", Number(e.target.value))}
                />
              </Field>
              <Field label="top_p" half>
                <input
                  type="number"
                  step="0.05"
                  value={p.top_p}
                  onChange={(e) => set("top_p", Number(e.target.value))}
                />
              </Field>
            </div>
            <div className="row" style={{ gap: 10 }}>
              <Field label="top_k" half>
                <input
                  type="number"
                  value={p.top_k}
                  onChange={(e) => set("top_k", Number(e.target.value))}
                />
              </Field>
              <Field label="parallel" half>
                <input
                  type="number"
                  value={p.parallel}
                  onChange={(e) => set("parallel", Number(e.target.value))}
                />
              </Field>
            </div>
            <Field label="mem max (MiB)">
              <input
                type="number"
                value={p.mem_max_mb ?? ""}
                placeholder="cgroup MemoryMax"
                onChange={(e) => set("mem_max_mb", num(e.target.value) as never)}
              />
            </Field>
            <Field label="mem swap max (MiB)">
              <input
                type="number"
                value={p.mem_swap_max_mb ?? ""}
                placeholder="cgroup MemorySwapMax"
                onChange={(e) =>
                  set("mem_swap_max_mb", num(e.target.value) as never)
                }
              />
            </Field>
            <Field label="ubatch size">
              <input
                type="number"
                value={p.ubatch_size}
                onChange={(e) => set("ubatch_size", Number(e.target.value))}
              />
            </Field>
          </div>
          )}

          {/* Live fit */}
          <div className="card">
            <h3>LIVE FIT ESTIMATE</h3>
            {fitErr && <div className="dim" style={{ fontSize: 11 }}>{fitErr}</div>}
            {!fit && !fitErr && (
              <div className="dim" style={{ fontSize: 11 }}>editing…</div>
            )}
            {fit && (
              <>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <span className="badge mag">{fit.verdict}</span>
                  <span className={`badge ${verdictClass(fit.verdict)}`}>
                    {fit.verdict}
                  </span>
                </div>
                <table style={{ marginTop: 10 }}>
                  <tbody>
                    <tr>
                      <td>weights (VRAM)</td>
                      <td className="mono">{fit.weights_mb} MiB</td>
                    </tr>
                    {fit.weights_ram_mb > 0 && (
                      <tr>
                        <td>weights (RAM spilled)</td>
                        <td className="mono">{fit.weights_ram_mb} MiB</td>
                      </tr>
                    )}
                    <tr>
                      <td>kv cache</td>
                      <td className="mono">{fit.kv_mb} MiB</td>
                    </tr>
                    <tr>
                      <td>buffers</td>
                      <td className="mono">{fit.buffers_mb} MiB</td>
                    </tr>
                    <tr>
                      <td>model VRAM</td>
                      <td className="mono magenta">{fit.model_vram_mb} MiB</td>
                    </tr>
                    <tr>
                      <td>desktop reserve</td>
                      <td className="mono">{fit.overhead_mb} MiB</td>
                    </tr>
                    <tr>
                      <td>available for model</td>
                      <td className="mono">{fit.available_for_model_mb} MiB</td>
                    </tr>
                  </tbody>
                </table>
              </>
            )}
          </div>

          {/* Test */}
          <div className="card">
            <h3>TEST LOAD</h3>
            <div className="dim" style={{ fontSize: 11, marginBottom: 8 }}>
              launches the loadout on a test port (live service paused), watching for
              OOM / crash / serve.
            </div>
            <div className="row" style={{ gap: 10 }}>
              <button className="action" onClick={runTest} disabled={testing}>
                {testing ? "TESTING…" : "TEST LOAD"}
              </button>
              {testing && (
                <button className="ghost" onClick={() => api.testStop()}>
                  STOP
                </button>
              )}
            </div>
            <div className="dim" style={{ fontSize: 11, margin: "8px 0 4px" }}>
              phase: {phase || "idle"}
            </div>
            <div className="term" ref={testRef} style={{ maxHeight: 200 }}>
              {log.length === 0 ? (
                <span className="dim">test output streams here…</span>
              ) : (
                log.map((l, i) => <div key={i}>{l}</div>)
              )}
            </div>
            {result && (
              <div
                className={`badge ${verdictClass(result.verdict)}`}
                style={{ marginTop: 8 }}
              >
                {result.verdict} — {result.summary}
              </div>
            )}
          </div>
        </div>

        {/* Unit preview */}
        {unit && (
          <div style={{ marginTop: 14 }}>
            <div className="view-title">RENDERED UNIT</div>
            <pre className="unit">{unit}</pre>
          </div>
        )}

        <div className="row" style={{ gap: 10, marginTop: 16 }}>
          <button className="action" onClick={save}>
            SAVE
          </button>
          <button className="ghost" onClick={onClose}>
            CLOSE
          </button>
          {msg && <span className="dim" style={{ fontSize: 11 }}>{msg}</span>}
        </div>
      </div>
    </div>
  );
}

function Field({
  label,
  half,
  children,
}: {
  label: string;
  half?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div
      className="field-wrap"
      style={half ? { flex: 1, minWidth: 120 } : { width: "100%" }}
    >
      <div className="field" style={{ marginBottom: 4 }}>
        {label}
      </div>
      {children}
    </div>
  );
}

export { defaultProfile };
