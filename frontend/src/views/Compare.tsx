import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import { useEngineList } from "../lib/engines";

/**
 * COMPARE — blind A/B over candidates. Pick models (checkboxes), one engine,
 * and a workload; the backend runs every trial, scores them, and reveals the
 * winner under opaque trial ids. "Use live server" samples the engine's UP
 * slot with no second boot (single-GPU safe); unchecked, cells boot
 * headlessly on the test ports.
 */
export default function Compare() {
  const [models, setModels] = useState<api.ModelRow[]>([]);
  const [workloads, setWorkloads] = useState<api.Workload[]>([]);
  const engines = useEngineList("LocalPath");
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [engine, setEngine] = useState("uncensored");
  const [workload, setWorkload] = useState("coding");
  const [extraTasks, setExtraTasks] = useState("");
  const [runs, setRuns] = useState(1);
  const [maxTokens, setMaxTokens] = useState(256);
  const [seed, setSeed] = useState(1);
  const [useLive, setUseLive] = useState(true);
  const [liveAddr, setLiveAddr] = useState("");
  const [report, setReport] = useState<api.CompareReport | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [steps, setSteps] = useState<string[]>([]);
  const logRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    api.listModels().then(setModels).catch(() => setModels([]));
    api.workloadsList().then(setWorkloads).catch(() => setWorkloads([]));
  }, []);

  // Live trial heartbeat from the backend — proves the run is alive during
  // multi-minute grids.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void listen<{ step: string }>("compare-step", (e) => {
      setSteps((prev) => [...prev.slice(-99), e.payload.step]);
    }).then((u) => { unlisten = u; });
    return () => { unlisten?.(); };
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [steps]);

  const toggle = (path: string) => {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const canRun =
    checked.size > 0 && engine.trim().length > 0 && runs >= 1 && maxTokens >= 1 && !loading;

  const refresh = async () => {
    if (!canRun) return;
    setReport(null);
    setError("");
    setSteps([]);
    setLoading(true);
    try {
      const extra = extraTasks
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      setReport(
        await api.compareRun({
          models: [...checked],
          engines: [engine],
          ollama: [],
          tasks: extra,
          runs,
          maxTokens,
          seed,
          live: useLive ? liveAddr.trim() || "auto" : null,
          workload: workload || null,
        }),
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div>
      <div className="view-title">COMPARE</div>

      <div className="card" style={{ marginBottom: 12 }}>
        <div style={{ fontSize: 12, marginBottom: 6 }}>Candidates (models × {engine || "…"})</div>
        {models.length === 0 && <div className="dim" style={{ fontSize: 12 }}>No models indexed — scan ~/models first.</div>}
        <div style={{ display: "grid", gap: 4, maxHeight: 220, overflowY: "auto" }}>
          {models.map((m) => (
            <label key={m.path} style={{ fontSize: 12, display: "flex", gap: 8, alignItems: "baseline" }}>
              <input type="checkbox" checked={checked.has(m.path)} onChange={() => toggle(m.path)} />
              <span className="mono">{m.name}</span>
              <span className="dim">{m.quant ?? ""} · {m.footprint_gib.toFixed(1)} GiB</span>
            </label>
          ))}
        </div>
      </div>

      <div style={{ display: "flex", gap: 12, flexWrap: "wrap", alignItems: "center", marginBottom: 12 }}>
        <label style={{ fontSize: 12 }}>
          Engine{" "}
          <select value={engine} onChange={(e) => setEngine(e.target.value)}>
            {engines.map((e) => (
              <option key={e.id} value={e.id}>{e.display}</option>
            ))}
          </select>
        </label>
        <label style={{ fontSize: 12 }}>
          Workload{" "}
          <select value={workload} onChange={(e) => setWorkload(e.target.value)}>
            <option value="">(none — extra tasks only)</option>
            {workloads.map((w) => (
              <option key={w.id} value={w.id}>{w.id}</option>
            ))}
          </select>
        </label>
        <label style={{ fontSize: 12 }}>
          Runs{" "}
          <input type="number" value={runs} onChange={(e) => setRuns(parseInt(e.target.value, 10) || 0)} min={1} max={10} style={{ width: 60 }} />
        </label>
        <label style={{ fontSize: 12 }}>
          Max tokens{" "}
          <input type="number" value={maxTokens} onChange={(e) => setMaxTokens(parseInt(e.target.value, 10) || 0)} min={1} max={8192} style={{ width: 80 }} />
        </label>
        <label style={{ fontSize: 12 }}>
          Seed{" "}
          <input type="number" value={seed} onChange={(e) => setSeed(parseInt(e.target.value, 10) || 0)} min={1} style={{ width: 90 }} />
        </label>
      </div>

      <div style={{ display: "flex", gap: 12, flexWrap: "wrap", alignItems: "center", marginBottom: 12 }}>
        <label style={{ fontSize: 12, display: "flex", gap: 6, alignItems: "center" }}>
          <input type="checkbox" checked={useLive} onChange={(e) => setUseLive(e.target.checked)} />
          Use live server (no second boot)
        </label>
        {useLive && (
          <label style={{ fontSize: 12 }}>
            Custom{" "}
            <input
              type="text"
              value={liveAddr}
              onChange={(e) => setLiveAddr(e.target.value)}
              placeholder="auto (engine slot)"
              style={{ width: 180 }}
            />
          </label>
        )}
        <button className="action" onClick={() => void refresh()} disabled={!canRun}>
          {loading ? "Running…" : "Run Comparison"}
        </button>
      </div>

      <div style={{ marginBottom: 12 }}>
        <label style={{ fontSize: 12, display: "block", marginBottom: 4 }}>
          Extra tasks <span className="dim">(optional — the workload above already supplies tasks; one <span className="mono">label=prompt</span> per line, e.g. <span className="mono">haiku=Write a haiku about video memory</span>)</span>
        </label>
        <textarea
          value={extraTasks}
          onChange={(e) => setExtraTasks(e.target.value)}
          rows={2}
          style={{ width: "100%", maxWidth: 640, fontSize: 12 }}
          placeholder="haiku=Write a haiku about video memory"
        />
      </div>

      {!canRun && !loading && (
        <div className="dim" style={{ fontSize: 12, marginBottom: 12 }}>
          Check at least one model above to run.
        </div>
      )}
      {error && <div style={{ color: "var(--oom)", fontSize: 12, marginBottom: 12 }}>{error}</div>}

      {(loading || steps.length > 0) && (
        <div
          ref={logRef}
          className="mono"
          style={{
            fontSize: 11,
            background: "rgba(0,0,0,0.3)",
            border: "1px solid var(--dim2, #333)",
            borderRadius: 4,
            padding: "8px 10px",
            maxHeight: 160,
            overflowY: "auto",
            marginBottom: 12,
            whiteSpace: "pre-wrap",
          }}
        >
          {steps.length === 0 ? "starting…" : steps.join("\n")}
          {loading && <span className="cursor" />}
        </div>
      )}

      {!report && !error && !loading && (
        <div style={{ fontSize: 13, color: "var(--text-muted)" }}>
          Check models, pick a workload, run — the blind verdict names the winner.
        </div>
      )}

      {report && (
        <div>
          <div style={{ fontSize: 12, color: "var(--text-muted)", marginBottom: 4 }}>
            <strong>Verdict:</strong> {report.verdict}
          </div>
          <div style={{ marginTop: 8, fontSize: 12, color: "var(--text-muted)" }}>
            <strong>Trials:</strong> {report.trials.length} total
          </div>
          {report.candidates.map((c, i) => (
            <div key={c.trial} style={{ borderTop: i === 0 ? "2px solid var(--primary)" : "none", marginTop: 16, paddingTop: 16 }}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", marginBottom: 8 }}>
                <span style={{ fontSize: 14, fontWeight: 500 }}>
                  <span>{c.trial}</span> × {c.engine} / {c.model}
                </span>
                <span style={{ fontSize: 12, color: c.verdict ? "var(--best)" : "var(--text-muted)" }}>
                  {c.verdict || "—"}
                </span>
              </div>
              <div style={{ fontSize: 13 }}><strong>Runs OK:</strong> {c.ok_runs}/{c.trials}</div>
              <div style={{ fontSize: 13 }}><strong>Mean tok/s:</strong> {c.mean_tok_s?.toFixed(1) || "—"}</div>
              <div style={{ fontSize: 13 }}><strong>Mean score:</strong> {c.mean_score?.toFixed(3) || "—"}</div>
              {c.failure && (
                <div style={{ fontSize: 12, color: "var(--oom)", marginTop: 4, fontStyle: "italic" }}>
                  Failure: {c.failure}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
