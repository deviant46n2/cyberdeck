import { useEffect, useSyncExternalStore } from "react";
import * as dls from "../lib/dl";
import * as br from "../lib/br";

function fmtBytes(b: number): string {
  if (!b) return "—";
  if (b >= 2 ** 30) return (b / 2 ** 30).toFixed(2) + " GiB";
  return (b / 2 ** 20).toFixed(1) + " MiB";
}

function fmtSpeed(s: number): string {
  if (s < 1024) return "";
  if (s >= 2 ** 20) return `${(s / 2 ** 20).toFixed(1)} MiB/s`;
  return `${Math.round(s / 1024)} KiB/s`;
}

/** Steam-style download manager: per-item start/stop, priority reorder, and a
 * persistent list of active / queued / paused / finished transfers. */
export default function Downloads() {
  const items = useSyncExternalStore(dls.subscribe, dls.getSnapshot);

  useEffect(() => {
    dls.init();
  }, []);

  const active = items.filter((d) => d.status === "active");
  const queued = items.filter((d) => d.status === "queued");
  const paused = items.filter((d) => d.status === "paused");
  const finished = items.filter((d) => d.status === "done" || d.status === "error");

  if (items.length === 0) {
    return (
      <div className="view">
        <div className="view-title">DOWNLOADS</div>
        <div className="card dim" style={{ padding: 24, textAlign: "center" }}>
          no transfers — queue models from MARKET or BROWSE
        </div>
      </div>
    );
  }

  const row = (d: dls.DlEntry) => {
    const pct = d.total > 0 ? Math.min(100, (d.done / d.total) * 100) : null;
    const name = d.rfilename.split("/").pop() ?? d.key;
    const arrows = (
      <>
        <span className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} title="raise priority"
          onClick={() => dls.movePriority(d.key, -1)}>▲</span>
        <span className="ghost" style={{ fontSize: 10, padding: "2px 6px" }} title="lower priority"
          onClick={() => dls.movePriority(d.key, 1)}>▼</span>
      </>
    );
    return (
      <div key={d.key} className={`dl-row ${d.status}`}>
        <div className="row" style={{ gap: 8, alignItems: "center" }}>
          <span className="dl-name" title={d.key}>
            {name}
          </span>
          {d.status === "active" && (
            <>
              <span className="mono" style={{ fontSize: 9, color: "var(--magenta)" }}>DL</span>
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} title="STOP — park this transfer (resume later, same place)"
                onClick={() => dls.stop(d.key)}>
                STOP
              </button>
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px", borderColor: "var(--oom)", color: "var(--oom)" }} title="remove transfer and drop partial"
                onClick={() => void dls.discard(d.key)}>
                ✕
              </button>
            </>
          )}
          {d.status === "queued" && (
            <>
              <span className="mono" style={{ fontSize: 9, color: "var(--warn)" }}>QUEUED</span>
              {arrows}
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} title="remove from queue"
                onClick={() => void dls.discard(d.key)}>
                ✕
              </button>
            </>
          )}
          {d.status === "paused" && (
            <>
              <span className="mono" style={{ fontSize: 9, color: "var(--warn)" }}>PAUSED</span>
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} title="START — resume from the partial"
                onClick={() => dls.start(d.key)}>
                START
              </button>
              {arrows}
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px", borderColor: "var(--oom)", color: "var(--oom)" }} title="remove transfer and drop partial"
                onClick={() => void dls.discard(d.key)}>
                ✕
              </button>
            </>
          )}
          {d.status === "done" && (
            <>
              <span className="mono" style={{ fontSize: 9, color: "var(--pass)" }}>✓ SAVED</span>
              {d.path && (
                <button className="ghost" style={{ fontSize: 9, padding: "2px 7px", borderColor: "var(--magenta)", color: "var(--magenta)" }}
                  title={`headless test ${d.path} — derive + verify on test port, NOT applied`}
                  onClick={() => void br.startTest(d.path as string, "freetoken")}>
                  TEST
                </button>
              )}
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} title="dismiss row (file stays in the vault)"
                onClick={() => dls.removeEntry(d.key)}>
                ✕
              </button>
            </>
          )}
          {d.status === "error" && (
            <>
              <span className="mono" style={{ fontSize: 9, color: "var(--oom)" }}>FAILED</span>
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} title={d.err} onClick={() => dls.start(d.key)}>
                RETRY
              </button>
              <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} title="dismiss row" onClick={() => dls.removeEntry(d.key)}>
                ✕
              </button>
            </>
          )}
        </div>
        <div className={`dl-bar ${pct == null && d.status === "active" ? "indeterminate" : ""}`}>
          {pct != null && (
            <div className="dl-fill" style={{ width: `${pct}%` }} />
          )}
        </div>
        <div className="row" style={{ justifyContent: "space-between", marginTop: 3 }}>
          <span className="dl-meta">
            {d.status === "active" && pct != null && `${pct.toFixed(1)}% · `}
            {fmtBytes(d.done)}
            {d.total > 0 && ` / ${fmtBytes(d.total)}`}
            {d.status === "error" ? ` — ${d.err ?? "failed"}` : ""}
          </span>
          <span className="dl-meta">{d.status === "active" && ` ${fmtSpeed(d.speed)}`}</span>
        </div>
      </div>
    );
  };

  const section = (title: string, count: number, nodes: React.ReactNode[]) =>
    count > 0 ? (
      <div className="card" style={{ marginBottom: 12 }}>
        <div className="mono" style={{ fontSize: 9, letterSpacing: 1, marginBottom: 8, color: "var(--dim2)" }}>
          {title} · {count}
        </div>
        {nodes}
      </div>
    ) : null;

  return (
    <div className="view">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center", marginBottom: 12 }}>
        <div className="view-title" style={{ marginBottom: 0 }}>DOWNLOADS</div>
        {finished.length > 0 && (
          <button className="ghost" style={{ fontSize: 9, padding: "3px 8px" }} onClick={dls.clearFinished}>
            CLEAR FINISHED
          </button>
        )}
      </div>

      {section("DOWNLOADING", active.length, active.map(row))}

      {section("QUEUED · PRIORITY", queued.length, queued.map(row))}

      {section("PAUSED", paused.length, paused.map(row))}

      {section("FINISHED", finished.length, finished.map(row))}

      <div className="dim" style={{ fontSize: 10, marginTop: 4 }}>
        priority ▲▼ orders the queue (front = runs next) · STOP parks in place · START resumes from the partial
      </div>
    </div>
  );
}