import { useEffect, useState } from "react";
import * as api from "../api";

export default function Signals() {
  const [orgs, setOrgs] = useState<string[]>([]);
  const [news, setNews] = useState<api.SignalRow[] | null>(null);
  const [org, setOrg] = useState("");
  const [status, setStatus] = useState<string>("");

  const refresh = async () => setOrgs(await api.watchlist());

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const check = async () => {
    setStatus("polling HuggingFace…");
    setNews(null);
    try {
      const r = await api.signalsCheck(15);
      setNews(r);
      setStatus(
        r.length === 0
          ? "nothing new since last check"
          : `${r.length} new model(s) from watched orgs`
      );
    } catch (e) {
      setStatus(`check failed: ${String(e)}`);
    }
  };

  const add = async () => {
    if (!org) return;
    await api.watchAdd(org);
    setOrg("");
    refresh();
  };

  const remove = async (o: string) => {
    await api.watchRemove(o);
    refresh();
  };

  return (
    <>
      <div className="view-title">SIGNALS</div>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3>WATCHED ORGS</h3>
        <div className="row" style={{ marginTop: 8 }}>
          <div style={{ flex: 1, minWidth: 220 }}>
            <input
              type="text"
              placeholder="add org (e.g. microsoft)"
              value={org}
              onChange={(e) => setOrg(e.target.value)}
            />
          </div>
          <button className="action" onClick={add}>
            ADD
          </button>
          <button className="action" onClick={check}>
            CHECK NOW
          </button>
        </div>
        <div className="row" style={{ marginTop: 12 }}>
          {orgs.map((o) => (
            <span key={o} className="badge mag" style={{ display: "inline-flex", gap: 6 }}>
              {o}
              <span
                style={{ cursor: "pointer", color: "var(--magenta)" }}
                onClick={() => remove(o)}
              >
                ✕
              </span>
            </span>
          ))}
        </div>
        {status && (
          <div className="dim" style={{ marginTop: 10, fontSize: 11 }}>
            {status}
          </div>
        )}
      </div>

      {news && (
        <div className="card">
          <h3>NEW SINCE LAST CHECK</h3>
          {news.length === 0 ? (
            <div className="dim" style={{ fontSize: 12 }}>
              no new releases — you're caught up
            </div>
          ) : (
            <table>
              <thead>
                <tr>
                  <th>MODEL</th>
                  <th>AUTHOR</th>
                  <th>TASK</th>
                  <th>⬇</th>
                  <th>♥</th>
                  <th>CREATED</th>
                </tr>
              </thead>
              <tbody>
                {news.map((m) => (
                  <tr key={m.id}>
                    <td className="mono">{m.id}</td>
                    <td>{m.author}</td>
                    <td>{m.pipeline_tag ?? "—"}</td>
                    <td className="mono">{m.downloads}</td>
                    <td className="mono">{m.likes}</td>
                    <td className="dim">{m.created_at.slice(0, 10)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </>
  );
}
