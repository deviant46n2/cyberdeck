import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

const ENGINE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
  { engine: "Ollama", host: "127.0.0.1", port: 11434 },
];

const ENGINE_LABELS: { value: string; label: string }[] = [
  { value: "llamacpp", label: "llamacpp (:18000)" },
  { value: "freetoken", label: "freetoken (:1919)" },
  { value: "ollama", label: "ollama (:11434)" },
];

// Surfaced from the agent's skill directory (~/.config/opencode/skills).
const SKILLS: { id: string; name: string; description: string }[] = [
  { id: "containers", name: "Containers", description: "Container management skill" },
  { id: "dev-environments", name: "Dev Environments", description: "Development environment setup" },
  { id: "linux-admin", name: "Linux Admin", description: "System administration tasks" },
  { id: "security-hardening", name: "Security Hardening", description: "Security-related operations" },
  { id: "vibecoding", name: "Vibe Coding", description: "Rapid prototyping and MVP development" },
  { id: "deep-research", name: "Deep Research", description: "Agent skill: search→read→synthesize→report" },
  { id: "mcp-picker", name: "MCP Picker", description: "Select MCP servers per agent session" },
];

function fmtTime(at: number): string {
  if (!at) return "—";
  const d = new Date(at * 1000);
  return d.toLocaleString();
}

export default function Console({ unit }: { unit: string }) {
  const [status, setStatus] = useState<api.EngineStatus[]>([]);
  const [history, setHistory] = useState<api.BenchRow[]>([]);
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  // --- agent sessions (multiple concurrent) ---
  const [prompt, setPrompt] = useState("");
  const [dir, setDir] = useState("/home/deviant/Projects/cyberdeck");
  const [auto, setAuto] = useState(false);
  const [model, setModel] = useState("");
  const [engine, setEngine] = useState("llamacpp");
  const [sessions, setSessions] = useState<
    { id: string; prompt: string; log: string[]; running: boolean }[]
  >([]);
  const sessionsRef = useRef<HTMLDivElement>(null);

  const refresh = () => {
    api.benchHistory().then(setHistory).catch(() => {});
    Promise.all(
      ENGINE_NODES.map((n) =>
        api.engineStatus(n.engine, n.host, n.port).catch(() => null)
      )
    ).then((res) => setStatus(res.filter(Boolean) as api.EngineStatus[]));
  };

  useEffect(refresh, []);

  // Stream opencode output into the matching session log.
  useEffect(() => {
    const started = listen<{ id: string; prompt: string }>(
      "opencode-started",
      (e) =>
        setSessions((s) => [
          ...s,
          { id: e.payload.id, prompt: e.payload.prompt, log: [], running: true },
        ])
    );
    const out = listen<{ session: string; stream: string; text: string }>(
      "opencode-output",
      (e) =>
        setSessions((s) =>
          s.map((x) =>
            x.id === e.payload.session
              ? { ...x, log: [...x.log, e.payload.text] }
              : x
          )
        )
    );
    const done = listen<{ session: string; code: number }>(
      "opencode-done",
      (e) =>
        setSessions((s) =>
          s.map((x) =>
            x.id === e.payload.session ? { ...x, running: false } : x
          )
        )
    );
    return () => {
      started.then((f) => f());
      out.then((f) => f());
      done.then((f) => f());
    };
  }, []);

  // Auto-scroll every session terminal.
  useEffect(() => {
    sessionsRef.current
      ?.querySelectorAll<HTMLDivElement>(".term")
      .forEach((el) => (el.scrollTop = el.scrollHeight));
  }, [sessions]);

  const bench = async (node: { engine: string; host: string; port: number }) => {
    setBusy(true);
    setMsg(`probing ${node.engine} :${node.port} …`);
    try {
      const row = await api.benchNow({
        engine: node.engine,
        host: node.host,
        port: node.port,
        model: "?",
        ctx: 0,
      });
      setMsg(`measured ${row.tps.toFixed(1)} tok/s`);
      refresh();
    } catch (e) {
      setMsg(`bench failed: ${String(e)}`);
    }
    setBusy(false);
  };

  const runAgent = async () => {
    if (!prompt.trim()) return;
    try {
      await api.opencodeRun({ prompt, dir, auto, model, engine });
    } catch (e) {
      setMsg(`failed to start agent: ${String(e)}`);
    }
  };

  const stopAgent = async (id: string) => {
    await api.opencodeStop(id);
  };

  const dismissAgent = (id: string) => {
    setSessions((s) => s.filter((x) => x.id !== id));
  };

  const copy = () => {
    if (unit) navigator.clipboard?.writeText(unit);
  };

  return (
    <>
      <div className="view-title">CONSOLE</div>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3>AGENT (opencode)</h3>
        <div className="field" style={{ marginBottom: 6 }}>TASK</div>
        <textarea
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="e.g. add a CLI flag to deck fit that prints the KV cache size in GiB"
          rows={3}
          style={{ width: "100%", fontFamily: "inherit", background: "#0a0a12", color: "#e8e8f0" }}
        />
        <div className="row" style={{ gap: 12, marginTop: 8, flexWrap: "wrap" }}>
          <div style={{ flex: 1, minWidth: 240 }}>
            <div className="field" style={{ marginBottom: 4 }}>PROJECT DIR</div>
            <input type="text" value={dir} onChange={(e) => setDir(e.target.value)} />
          </div>
          <div style={{ width: 180 }}>
            <div className="field" style={{ marginBottom: 4 }}>ENGINE</div>
            <select
              value={engine}
              onChange={(e) => setEngine(e.target.value)}
              style={{ width: "100%", fontFamily: "inherit" }}
            >
              {ENGINE_LABELS.map((o) => (
                <option key={o.value} value={o.value}>{o.label}</option>
              ))}
            </select>
          </div>
          <div style={{ width: 160 }}>
            <div className="field" style={{ marginBottom: 4 }}>MODEL (optional)</div>
            <input type="text" value={model} onChange={(e) => setModel(e.target.value)} placeholder="provider/model" />
          </div>
        </div>
        <label className="row" style={{ gap: 8, marginTop: 8, fontSize: 12 }}>
          <input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} />
          <span>
            auto-approve permissions (<span style={{ color: "var(--oom)" }}>--auto: agent may modify files unprompted</span>)
          </span>
        </label>
        <div className="row" style={{ gap: 10, marginTop: 10 }}>
          <button className="action" onClick={runAgent} disabled={!prompt.trim()}>
            RUN AGENT
          </button>
          <span className="dim" style={{ fontSize: 11 }}>
            {sessions.length > 0
              ? `${sessions.length} session${sessions.length > 1 ? "s" : ""} · ${
                  sessions.filter((s) => s.running).length
                } running`
              : "sessions run concurrently — each gets its own log"}
          </span>
        </div>

        <div ref={sessionsRef} style={{ marginTop: 12, display: "grid", gap: 12 }}>
          {sessions.length === 0 && (
            <div className="dim" style={{ fontSize: 11 }}>
              no agent sessions yet — hit RUN AGENT
            </div>
          )}
          {sessions.map((s) => (
            <div key={s.id} className="card" style={{ background: "#07070e" }}>
              <div
                className="row"
                style={{ justifyContent: "space-between", gap: 10, marginBottom: 8 }}
              >
                <span
                  className="mono"
                  style={{ fontSize: 11, color: "var(--magenta)", flex: 1 }}
                >
                  {s.prompt.length > 80 ? s.prompt.slice(0, 80) + "…" : s.prompt}
                </span>
                <span className={`dot ${s.running ? "up" : "down"}`} />
                <span className="dim" style={{ fontSize: 10 }}>
                  {s.running ? "running" : "done"}
                </span>
              </div>
              <div className="term" style={{ maxHeight: 220 }}>
                {s.log.length === 0 ? (
                  <span className="dim">starting…</span>
                ) : (
                  s.log.map((l, i) => <div key={i}>{l}</div>)
                )}
              </div>
              <div className="row" style={{ gap: 8, marginTop: 8 }}>
                {s.running ? (
                  <button className="ghost" onClick={() => stopAgent(s.id)}>
                    STOP
                  </button>
                ) : (
                  <button className="ghost" onClick={() => dismissAgent(s.id)}>
                    DISMISS
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3>AVAILABLE SKILLS (opencode)</h3>
        <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
          {SKILLS.map((s) => (
            <span
              key={s.id}
              className="mono"
              title={s.description}
              style={{ fontSize: 11, border: "1px solid var(--dim2)", padding: "3px 8px", borderRadius: 3 }}
            >
              {s.name}
            </span>
          ))}
        </div>
      </div>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3>ENGINE TELEMETRY</h3>
        <div className="row" style={{ gap: 24, flexWrap: "wrap" }}>
          {ENGINE_NODES.map((n) => {
            const s = status.find((x) => x.engine === n.engine);
            return (
              <div key={n.engine} className="card" style={{ minWidth: 220 }}>
                <div className="row" style={{ justifyContent: "space-between" }}>
                  <span className="mono" style={{ fontSize: 12 }}>
                    {n.engine === "LlamaCpp" ? "llamacpp" : n.engine === "FreeToken" ? "freetoken" : "ollama"} :{n.port}
                  </span>
                  <span className={`dot ${s?.up ? "up" : "down"}`} />
                </div>
                <div className="dim" style={{ fontSize: 11, margin: "6px 0 10px" }}>
                  {s ? (s.up ? "ONLINE" : "offline") : "…"}
                </div>
                <button
                  className="ghost"
                  disabled={busy || !s?.up}
                  onClick={() => bench(n)}
                >
                  BENCH tok/s
                </button>
              </div>
            );
          })}
        </div>
        {msg && (
          <div className="dim" style={{ marginTop: 10, fontSize: 11 }}>
            {msg}
          </div>
        )}
      </div>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3>BENCH HISTORY</h3>
        {history.length === 0 ? (
          <div className="dim" style={{ fontSize: 11 }}>
            no measurements yet — hit BENCH on a live engine
          </div>
        ) : (
          <table>
            <thead>
              <tr>
                <th>ENGINE</th>
                <th>PORT</th>
                <th>TOK/S</th>
                <th>WHEN</th>
              </tr>
            </thead>
            <tbody>
              {history.map((b) => (
                <tr key={b.id}>
                  <td>{b.engine === "LlamaCpp" ? "llamacpp" : b.engine === "FreeToken" ? "freetoken" : "ollama"}</td>
                  <td className="mono">:{b.port}</td>
                  <td className="mono magenta">{b.tps.toFixed(1)}</td>
                  <td className="dim" style={{ fontSize: 11 }}>{fmtTime(b.at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="view-title">LAST RENDERED UNIT</div>
      <div className="dim" style={{ fontSize: 11, marginBottom: 10 }}>
        from LOADOUTS preview / apply
      </div>
      {unit ? (
        <>
          <pre className="unit">{unit}</pre>
          <div className="row" style={{ marginTop: 10 }}>
            <button className="ghost" onClick={copy}>
              COPY
            </button>
          </div>
        </>
      ) : (
        <div className="stub">no unit rendered yet</div>
      )}
    </>
  );
}