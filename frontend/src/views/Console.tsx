import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

const ENGINE_NODES: { engine: string; host: string; port: number }[] = [
  { engine: "LlamaCpp", host: "127.0.0.1", port: 18000 },
  { engine: "FreeToken", host: "127.0.0.1", port: 1919 },
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

  // --- agent session state ---
  const [prompt, setPrompt] = useState("");
  const [dir, setDir] = useState("/home/deviant/Projects/cyberdeck");
  const [auto, setAuto] = useState(false);
  const [model, setModel] = useState("");
  const [running, setRunning] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const logRef = useRef<HTMLDivElement>(null);

  const refresh = () => {
    api.benchHistory().then(setHistory).catch(() => {});
    Promise.all(
      ENGINE_NODES.map((n) =>
        api.engineStatus(n.engine, n.host, n.port).catch(() => null)
      )
    ).then((res) => setStatus(res.filter(Boolean) as api.EngineStatus[]));
  };

  useEffect(refresh, []);

  // Stream opencode output into the log.
  useEffect(() => {
    const un = listen<{ stream: string; text: string }>("opencode-output", (e) => {
      setLog((l) => [...l, e.payload.text]);
    });
    const done = listen<{ code: number }>("opencode-done", () => {
      setRunning(false);
    });
    return () => {
      un.then((f) => f());
      done.then((f) => f());
    };
  }, []);

  // Auto-scroll the log.
  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [log]);

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
    if (!prompt.trim() || running) return;
    setLog([]);
    setRunning(true);
    try {
      await api.opencodeRun({ prompt, dir, auto, model });
    } catch (e) {
      setLog((l) => [...l, `✗ ${String(e)}`]);
      setRunning(false);
    }
  };

  const stopAgent = async () => {
    await api.opencodeStop();
    setLog((l) => [...l, "— interrupted —"]);
    setRunning(false);
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
          <button className="action" onClick={runAgent} disabled={running}>
            {running ? "RUNNING…" : "RUN AGENT"}
          </button>
          {running && (
            <button className="ghost" onClick={stopAgent}>
              STOP
            </button>
          )}
        </div>
        <div className="term" ref={logRef}>
          {log.length === 0 ? (
            <span className="dim">agent output streams here…</span>
          ) : (
            log.map((l, i) => <div key={i}>{l}</div>)
          )}
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
                    {n.engine === "LlamaCpp" ? "llamacpp" : "freetoken"} :{n.port}
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
                  <td>{b.engine === "LlamaCpp" ? "llamacpp" : "freetoken"}</td>
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
