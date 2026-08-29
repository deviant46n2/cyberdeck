import { useState, useEffect } from "react";
import * as api from "../api";

/** Shorten a model path to its filename for the scoreboard. */
function shortModel(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] ?? path;
}

function fmtTps(tps: number | null): string {
  return tps != null ? tps.toFixed(1) : "—";
}

function ago(ts: number): string {
  if (ts <= 0) return "—";
  const s = Math.floor(Date.now() / 1000) - ts;
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export default function Bench() {
  const [rows, setRows] = useState<api.BenchRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState("");

  const fetchHistory = async () => {
    try {
      const r = await api.benchHistory();
      setRows(r);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void fetchHistory();
  }, []);

  const recordNow = async (engine: string, host: string, port: number, model: string, ctx: number) => {
    setRecording(true);
    setError("");
    try {
      const row = await api.benchNow({ engine, host, port, model, ctx });
      setRows((prev) => [row, ...prev]);
    } catch (e) {
      setError(String(e));
    } finally {
      setRecording(false);
    }
  };

  // Group rows by (model, engine) for the scoreboard table
  const groups = new Map<string, api.BenchRow[]>();
  for (const r of rows) {
    const key = `${r.model}\x00${r.engine}`;
    const list = groups.get(key);
    if (list) list.push(r);
    else groups.set(key, [r]);
  }

  const scoreboard: Array<{
    model: string;
    engine: string;
    best: number;
    latest: number;
    avg: number;
    count: number;
  }> = [];
  for (const [, list] of groups) {
    const tps = list.map((r) => r.tps);
    const best = Math.max(...tps);
    const latest = list[0].tps;
    const avg = tps.reduce((a, b) => a + b, 0) / tps.length;
    scoreboard.push({
      model: list[0].model,
      engine: list[0].engine,
      best,
      latest,
      avg,
      count: list.length,
    });
  }
  scoreboard.sort((a, b) => b.best - a.best);

  return (
    <div className="bench-view">
      <div className="bench-header">
        <h2>BENCH</h2>
        <span className="dim" style={{ fontSize: 11 }}>
          tok/s throughput scoreboard
        </span>
      </div>

      {error && (
        <div className="mono" style={{ fontSize: 11, color: "var(--oom)", marginBottom: 8 }}>
          {error}
        </div>
      )}

      {scoreboard.length > 0 && (
        <div className="card" style={{ padding: 0, marginBottom: 16, overflow: "hidden" }}>
          <div className="mono" style={{
            fontSize: 9, letterSpacing: 1, padding: "8px 12px",
            borderBottom: "1px solid var(--line)", color: "var(--dim2)"
          }}>
            BEST TOK/S BY MODEL × ENGINE
          </div>
          <table className="score-table">
            <thead>
              <tr>
                <th>model</th>
                <th>engine</th>
                <th className="num">best</th>
                <th className="num">latest</th>
                <th className="num">avg</th>
                <th className="num">runs</th>
              </tr>
            </thead>
            <tbody>
              {scoreboard.map((g) => (
                <tr key={`${g.model}\x00${g.engine}`}>
                  <td className="mono" style={{ fontSize: 11 }}>{shortModel(g.model)}</td>
                  <td>{g.engine}</td>
                  <td className="num mono" style={{ color: "var(--pass)" }}>{fmtTps(g.best)}</td>
                  <td className="num mono">{fmtTps(g.latest)}</td>
                  <td className="num mono">{fmtTps(g.avg)}</td>
                  <td className="num">{g.count}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Raw history */}
      <div className="card" style={{ padding: 0, marginBottom: 16 }}>
        <div className="mono" style={{
          fontSize: 9, letterSpacing: 1, padding: "8px 12px",
          borderBottom: "1px solid var(--line)", color: "var(--dim2)"
        }}>
          RECENT READINGS
        </div>
        {loading ? (
          <div className="mono" style={{ padding: 16, color: "var(--dim2)", fontSize: 11 }}>
            loading…
          </div>
        ) : rows.length === 0 ? (
          <div className="mono" style={{ padding: 16, color: "var(--dim2)", fontSize: 11 }}>
            no benchmark readings yet — record one above
          </div>
        ) : (
          <table className="score-table">
            <thead>
              <tr>
                <th>model</th>
                <th>engine</th>
                <th>host</th>
                <th className="num">ctx</th>
                <th className="num">tok/s</th>
                <th>at</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id}>
                  <td className="mono" style={{ fontSize: 11 }}>{shortModel(r.model)}</td>
                  <td>{r.engine}</td>
                  <td className="mono" style={{ fontSize: 11 }}>{r.host}:{r.port}</td>
                  <td className="num">{r.ctx}</td>
                  <td className="num mono" style={{ color: "var(--pass)" }}>{fmtTps(r.tps)}</td>
                  <td className="dim" style={{ fontSize: 11 }}>{ago(r.at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Quick record form */}
      <div className="card" style={{ padding: 12 }}>
        <div className="mono" style={{ fontSize: 9, letterSpacing: 1, marginBottom: 8, color: "var(--dim2)" }}>
          RECORD NOW
        </div>
        <RecordForm onRecord={recordNow} recording={recording} />
      </div>
    </div>
  );
}

function RecordForm({
  onRecord,
  recording,
}: {
  onRecord: (engine: string, host: string, port: number, model: string, ctx: number) => void;
  recording: boolean;
}) {
  const [engine, setEngine] = useState("llamacpp");
  const [host, setHost] = useState("127.0.0.1");
  const [port, setPort] = useState("18000");
  const [model, setModel] = useState("");
  const [ctx, setCtx] = useState("32768");

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const p = parseInt(port, 10);
    const c = parseInt(ctx, 10);
    if (!model.trim()) return;
    onRecord(engine, host, isNaN(p) ? 18000 : p, model.trim(), isNaN(c) ? 32768 : c);
  };

  return (
    <form onSubmit={handleSubmit} style={{ display: "grid", gap: 6, gridTemplateColumns: "1fr 1fr", fontSize: 11 }}>
      <input className="input" placeholder="engine (llamacpp|freetoken)" value={engine}
        onChange={(e) => setEngine(e.target.value)} style={{ gridColumn: "1 / -1" }} />
      <input className="input" placeholder="host" value={host}
        onChange={(e) => setHost(e.target.value)} />
      <input className="input" placeholder="port" value={port}
        onChange={(e) => setPort(e.target.value)} />
      <input className="input" placeholder="model path" value={model}
        onChange={(e) => setModel(e.target.value)} style={{ gridColumn: "1 / -1" }} />
      <input className="input" placeholder="ctx" value={ctx}
        onChange={(e) => setCtx(e.target.value)} />
      <button className="ghost" type="submit" disabled={recording || !model.trim()}
        style={{ fontSize: 9, padding: "4px 10px", justifySelf: "start" }}>
        {recording ? "recording…" : "RECORD"}
      </button>
    </form>
  );
}
