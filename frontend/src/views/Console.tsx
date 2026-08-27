import { useEffect, useState } from "react";
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

  const refresh = () => {
    api.benchHistory().then(setHistory).catch(() => {});
    Promise.all(
      ENGINE_NODES.map((n) =>
        api.engineStatus(n.engine, n.host, n.port).catch(() => null)
      )
    ).then((res) => setStatus(res.filter(Boolean) as api.EngineStatus[]));
  };

  useEffect(refresh, []);

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

  const copy = () => {
    if (unit) navigator.clipboard?.writeText(unit);
  };

  return (
    <>
      <div className="view-title">CONSOLE</div>

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
