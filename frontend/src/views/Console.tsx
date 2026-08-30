import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

const COMPARE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
  { engine: "Ollama", host: "127.0.0.1", port: 11434 },
];

function fmtTime(at: number): string {
  if (!at) return "—";
  const d = new Date(at * 1000);
  return d.toLocaleString();
}

export default function Console() {
  const [report, setReport] = useState<api.CompareReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [seed, setSeed] = useState(1);
  const [tasks, setTasks] = useState<{ name: string; runs: number }[]>([]);
  const [maxTokens, setMaxTokens] = useState(128);
  const [bootTimeout, setBootTimeout] = useState(240000);
  const [skills, setSkills] = useState<any[]>([]);
  const compareRef = useRef<HTMLDivElement>(null);

  const refresh = () => {
    setReport(null);
    setLoading(true);
    api.compareRun({ seed, tasks, maxTokens, bootTimeout }).then(setReport).catch(() => {}).finally(() => setLoading(false));
  };

  useEffect(() => {
    refresh();
    const t = window.setInterval(() => void refresh(), 30000);
    return () => window.clearInterval(t);
  }, [seed, tasks, maxTokens, bootTimeout]);

  // Load skills from opencode skills directory
  useEffect(() => {
    // Check for skills directory - this is a client-side check
    // In production, skills are served from the backend
    const loadSkills = () => {
      // Simulate skills loading - in real implementation, this would
      // fetch from the opencode skills directory
      const defaultSkills = [
        { id: "containers", name: "Containers", description: "Container management skill" },
        { id: "dev-environments", name: "Dev Environments", description: "Development environment setup" },
        { id: "linux-admin", name: "Linux Admin", description: "System administration tasks" },
        { id: "security-hardening", name: "Security Hardening", description: "Security-related operations" },
        { id: "vibecoding", name: "Vibe Coding", description: "Rapid prototyping and MVP development" },
      ];
      setSkills(defaultSkills);
    };
    loadSkills();
  }, []);

  const handleSeedChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSeed(parseInt(e.target.value, 10));
  };

  const handleTasksChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const values = e.target.value.split(",").map((s: string) => s.trim()).filter((s: string) => s.length > 0);
    setTasks(values.map((name: string) => ({ name, runs: 5 })));
  };

  const handleMaxTokensChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setMaxTokens(parseInt(e.target.value, 10));
  };

  const handleBootTimeoutChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setBootTimeout(parseInt(e.target.value, 10));
  };

  useEffect(() => {
    const reportListener = listen<api.CompareReport>("compare_report", (e) => {
      setReport(e.payload);
    });
    reportListener.then((f) => f());
    return () => {};
  }, []);

  // Build candidate DOM strings
  const candidateNodes = report?.candidates.map((c, i) => (
    <div key={i} style={{ borderTop: i === 0 ? "2px solid var(--primary)" : "none", marginTop: 16, paddingTop: 16 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", marginBottom: 8 }}>
        <span style={{ fontSize: 14, fontWeight: 500 }}>
          <span>{c.trial}</span> × {c.engine} / {c.model}
        </span>
        <span style={{ fontSize: 12, color: c.verdict ? "var(--best)" : "var(--text-muted)" }}>
          {c.verdict || "—"}
        </span>
      </div>
      <div style={{ fontSize: 13, color: "var(--text)" }}>
        <strong>Context:</strong> {c.ctx} tokens
      </div>
      <div style={{ fontSize: 13, color: "var(--text)" }}>
        <strong>Runs OK:</strong> {c.ok_runs}/{c.trials}
      </div>
      <div style={{ fontSize: 13, color: "var(--text)" }}>
        <strong>Mean tok/s:</strong> {c.mean_tok_s?.toFixed(1) || "—"}
      </div>
      <div style={{ fontSize: 13, color: "var(--text)" }}>
        <strong>Mean score:</strong> {c.mean_score?.toFixed(3) || "—"}
      </div>
      {c.failure && (
        <div style={{ fontSize: 12, color: "var(--oom)", marginTop: 4, fontStyle: "italic" }}>
          Failure: {c.failure}
        </div>
      )}
    </div>
  )) || [];

  if (!report) {
    return (
      <div>
        <div className="view-title">COMPARE</div>
        <div style={{ padding: 24, maxHeight: 400 }}>
          <p style={{ fontSize: 14, color: "var(--text-muted)" }}>
            Click "Run Comparison" to benchmark models across residents blind.
          </p>
          <div style={{ marginTop: 16, display: "grid", gap: 16 }}>
            <div>
              <label style={{ fontSize: 12, marginBottom: 4, display: "block" }}>Seed</label>
              <input type="number" value={seed} onChange={handleSeedChange} min="1" max="999999" style={{ width: 100 }} />
            </div>
            <div>
              <label style={{ fontSize: 12, marginBottom: 4, display: "block" }}>Tasks</label>
              <input type="text" value={tasks.map((t) => t.name).join(",")} onChange={handleTasksChange} placeholder="task1,task2" style={{ width: 200 }} />
              <span className="dim" style={{ fontSize: 10 }}>(runs per task default to 5)</span>
            </div>
            <div>
              <label style={{ fontSize: 12, marginBottom: 4, display: "block" }}>Max tokens</label>
              <input type="number" value={maxTokens} onChange={handleMaxTokensChange} min="1" max="8192" style={{ width: 100 }} />
            </div>
            <div>
              <label style={{ fontSize: 12, marginBottom: 4, display: "block" }}>Boot timeout (ms)</label>
              <input type="number" value={bootTimeout} onChange={handleBootTimeoutChange} min="30000" max="600000" style={{ width: 100 }} />
            </div>
            <button className="action" onClick={refresh} disabled={loading}>
              {loading ? "Running…" : "Run Comparison"}
            </button>
          </div>
        </div>
        {/* Skills section */}
        <div style={{ marginTop: 24, fontSize: 12, color: "var(--text-muted)" }}>
          <strong>Available Skills:</strong>
        </div>
        <div style={{ marginTop: 8 }}>
          {skills.map((s) => (
            <div key={s.id} style={{ display: "flex", alignItems: "center", marginBottom: 4, fontSize: 12, color: "var(--text)" }}>
              <span style={{ flex: 1, marginRight: 8 }}>{s.name}</span>
              <span style={{ color: "var(--text-muted)", fontSize: 11 }}>{s.description}</span>
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div>
      <div className="view-title">COMPARE</div>
      <div style={{ marginTop: 24, fontSize: 12, color: "var(--text-muted)" }}>
        <strong>Candidates:</strong> {report?.candidates.length || 0}
      </div>
      <div style={{ marginTop: 24, fontSize: 12, color: "var(--text-muted)" }}>
        <strong>Verdict:</strong> {report?.verdict || "—"}
      </div>
      <div style={{ marginTop: 16 }}>
        {candidateNodes}
      </div>
      <div style={{ marginTop: 16, fontSize: 12, color: "var(--text-muted)" }}>
        <strong>Trials:</strong> {report?.trials.length || 0} total trials
      </div>
      <div style={{ marginTop: 16 }}>
        {candidateNodes}
      </div>
    </div>
  );
}
