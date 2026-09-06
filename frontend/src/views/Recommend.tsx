import { useEffect, useState } from "react";
import * as api from "../api";

const WORKLOADS = ["coding", "reasoning", "instruction", "assistant", "agent"];
const OBJECTIVES = ["quality", "speed", "efficient"];

/**
 * RECOMMEND — the loop's finish line. Ranks (model × engine) candidates from
 * measured `matrix_runs` evidence for a workload: task success from recorded
 * evaluations, tok/s p50 from real trials. Proxy-scored trials are labeled,
 * never hidden — see the explain line per candidate.
 */
export default function Recommend() {
  const [workload, setWorkload] = useState("coding");
  const [objective, setObjective] = useState("quality");
  const [ranked, setRanked] = useState<api.RankedCandidate[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = async (w: string, o: string) => {
    setLoading(true);
    setError("");
    try {
      setRanked(await api.recommend(w, o));
    } catch (e) {
      setError(String(e));
      setRanked(null);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh(workload, objective);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workload, objective]);

  return (
    <div>
      <div className="view-title">RECOMMEND</div>
      <div style={{ display: "flex", gap: 12, marginBottom: 12, alignItems: "center" }}>
        <label style={{ fontSize: 12 }}>
          Workload{" "}
          <select value={workload} onChange={(e) => setWorkload(e.target.value)}>
            {WORKLOADS.map((w) => (
              <option key={w} value={w}>{w}</option>
            ))}
          </select>
        </label>
        <label style={{ fontSize: 12 }}>
          Objective{" "}
          <select value={objective} onChange={(e) => setObjective(e.target.value)}>
            {OBJECTIVES.map((o) => (
              <option key={o} value={o}>{o}</option>
            ))}
          </select>
        </label>
        <button className="ghost" onClick={() => void refresh(workload, objective)} disabled={loading}>
          {loading ? "Ranking…" : "Refresh"}
        </button>
      </div>
      {error && <div style={{ color: "var(--oom)", fontSize: 12 }}>{error}</div>}
      {!error && ranked && ranked.length === 0 && (
        <div className="dim" style={{ fontSize: 13 }}>
          No measured trials for workload “{workload}” yet — run{" "}
          <span className="mono">deck bench matrix --workload {workload} [--live host:port]</span>{" "}
          or a blind Compare first.
        </div>
      )}
      {!error && ranked && ranked.length > 0 && (
        <div className="card">
          <table>
            <thead>
              <tr>
                <th>#</th>
                <th>MODEL</th>
                <th>ENGINE</th>
                <th>SUCCESS</th>
                <th>P50 TOK/S</th>
                <th>RUNS</th>
                <th>WHY</th>
              </tr>
            </thead>
            <tbody>
              {ranked.map((c, i) => (
                <tr key={`${c.model}\x00${c.engine}`} style={i === 0 ? { background: "rgba(63,185,80,0.08)" } : undefined}>
                  <td className="mono">{i + 1}</td>
                  <td style={{ maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{c.model}</td>
                  <td>{c.engine}</td>
                  <td className="mono">{(c.success_rate * 100).toFixed(0)}%</td>
                  <td className="mono">{c.p50_tok_s != null ? c.p50_tok_s.toFixed(1) : "—"}</td>
                  <td className="mono">{c.runs}</td>
                  <td className="dim" style={{ fontSize: 11 }}>{c.explain}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
