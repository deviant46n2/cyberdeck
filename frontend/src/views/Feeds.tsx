import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import { LAST_SEEN_KEY, isNewSince, parseLastSeen } from "../lib/feeds";

const SCORE_COLOR = (s: number) => (s >= 0.65 ? "var(--pass)" : s >= 0.4 ? "var(--warn)" : "var(--dim2)");

/** FEEDS — online intelligence lane (O4).
 *
 * Surfaces the release catalog ranked by relevance to *this* hardware and
 * installed fleet — `deck feeds rank` in the UI. POLL pulls new releases from
 * the HF/GitHub adapters; the workload hint reweights family overlap so
 * "what should I care about" becomes "what's worth testing for THIS workload
 * on THIS 5070 Ti" instead of "what's popular". Self-fetching, no global state.
 */
/** O4 recency gate (2026-08-30): a `feeds.last_seen` epoch-seconds setting
 * (read/written through the audit-tracked O3 settings store) marks releases
 * newly entered into the catalog since you last acknowledged them. NEW is
 * computed locally as `fetched_at > last_seen`; it only ever advances via the
 * explicit "MARK SEEN" action, so the "what changed since I last looked" list
 * is persistent, not wiped by a routine poll. Parsing/compare helpers live in
 * src/lib/feeds.ts (unit-tested); this view is a thin consumer. */

export default function Feeds() {
  const [ranked, setRanked] = useState<api.ScoredRelease[] | null>(null);
  const [workload, setWorkload] = useState<string>("");
  const [status, setStatus] = useState("");
  const [polling, setPolling] = useState(false);
  const [limit, setLimit] = useState(20);
  const [lastSeen, setLastSeen] = useState<number>(0);

  const newCount = ranked?.filter((r) => isNewSince(r.release.fetched_at, lastSeen)).length ?? 0;

  const load = useCallback(async (wl: string, lim: number) => {
    setStatus("ranking…");
    try {
      const r = await api.feedsRank(lim, wl || null);
      setRanked(r);
      setStatus(
        r.length === 0
          ? "no releases yet — run POLL first"
          : `${r.length} release(s) ranked for ${wl || "hardware default"}`
      );
    } catch (e) {
      setStatus(`rank failed: ${String(e)}`);
    }
  }, []);

  useEffect(() => {
    api.settingsGet(LAST_SEEN_KEY).then((raw) => setLastSeen(parseLastSeen(raw))).catch(() => {});
    load(workload, limit);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const poll = async () => {
    setPolling(true);
    setStatus("polling HF + GitHub…");
    try {
      const r = await api.feedsPoll([]);
      setStatus(`polled: ${r.fetched} fetched, ${r.inserted} new`);
      await load(workload, limit);
    } catch (e) {
      setStatus(`poll failed: ${String(e)}`);
    } finally {
      setPolling(false);
    }
  };

  const changeWorkload = (wl: string) => {
    setWorkload(wl);
    load(wl, limit);
  };

  const markSeen = async () => {
    const now = Math.floor(Date.now() / 1000);
    try {
      await api.settingsSet(LAST_SEEN_KEY, String(now), "feeds: mark all seen", "ui");
      setLastSeen(now);
      setStatus("all releases marked seen — NEW markers cleared until the next poll");
    } catch (e) {
      setStatus(`mark seen failed: ${String(e)}`);
    }
  };

  return (
    <>
      <div className="view-title">FEEDS</div>

      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row" style={{ gap: 10, flexWrap: "wrap", alignItems: "center" }}>
          <button className="action" onClick={poll} disabled={polling}>
            {polling ? "POLLING…" : "POLL"}
          </button>
          {newCount > 0 && (
            <span className="badge mag" title="new since you last marked seen">
              {newCount} NEW
            </span>
          )}
          <button className="ghost" onClick={markSeen} disabled={newCount === 0}>
            MARK SEEN
          </button>
          <label className="dim" style={{ fontSize: 11 }}>
            workload
          </label>
          <select
            value={workload}
            onChange={(e) => changeWorkload(e.target.value)}
            style={{
              fontSize: 11,
              background: "var(--panel-2)",
              color: "var(--text)",
              border: "1px solid var(--dim2)",
              borderRadius: 4,
              padding: "3px 8px",
            }}
          >
            <option value="">hardware default</option>
            <option value="coding">coding</option>
            <option value="reasoning">reasoning</option>
            <option value="instruction">instruction</option>
            <option value="assistant">assistant</option>
          </select>
          <select
            value={limit}
            onChange={(e) => { setLimit(Number(e.target.value)); load(workload, Number(e.target.value)); }}
            style={{
              fontSize: 11,
              background: "var(--panel-2)",
              color: "var(--text)",
              border: "1px solid var(--dim2)",
              borderRadius: 4,
              padding: "3px 8px",
            }}
          >
            {[10, 20, 50].map((n) => (
              <option key={n} value={n}>{n}</option>
            ))}
          </select>
          <span className="dim" style={{ fontSize: 10, flex: 1, textAlign: "right" }}>
            hardware-grounded relevance · fit · novelty · bench delta
          </span>
        </div>
        {status && (
          <div className="dim" style={{ marginTop: 8, fontSize: 11 }}>
            {status}
          </div>
        )}
      </div>

      {ranked && (
        <div className="card">
          <table>
            <thead>
              <tr>
                <th>NEW</th>
                <th>#</th>
                <th>SCORE</th>
                <th>FITS</th>
                <th>DISK</th>
                <th>MAX CTX</th>
                <th>SRC</th>
                <th>REPO / REV</th>
                <th>WHY</th>
              </tr>
            </thead>
            <tbody>
              {ranked.length === 0 && (
                <tr>
                  <td colSpan={9} className="dim">rank the catalog by polling first</td>
                </tr>
              )}
              {ranked.map((r, i) => {
                const isNew = isNewSince(r.release.fetched_at, lastSeen);
                return (
                  <tr key={`${r.release.source}:${r.release.repo}@${r.release.rev}`}>
                    <td>
                      {isNew && <span className="badge mag">NEW</span>}
                    </td>
                    <td className="mono dim">{i + 1}</td>
                    <td className="mono" style={{ color: SCORE_COLOR(r.score.total) }}>
                      {r.score.total.toFixed(2)}
                    </td>
                    <td className="mono" style={{ color: r.score.fits ? "var(--pass)" : "var(--oom)" }}>
                      {r.score.fits ? "✓" : "✗"}
                    </td>
                    <td className="mono dim">
                      {r.score.disk_gb != null ? `~${r.score.disk_gb.toFixed(0)}G` : "-"}
                    </td>
                    <td className="mono dim">
                      {r.score.max_ctx != null ? `@${r.score.max_ctx}` : "-"}
                    </td>
                    <td className="mono dim">{r.release.source}</td>
                    <td className="mono" style={{ maxWidth: 300, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      <a href={r.release.url} target="_blank" rel="noreferrer" style={{ color: "var(--cyan)" }}>
                        {r.release.repo}
                      </a>
                      <span className="dim">@{r.release.rev}</span>
                    </td>
                    <td className="dim" style={{ fontSize: 10 }}>
                      {r.score.reasons.join(", ")}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
