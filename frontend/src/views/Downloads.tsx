import { useEffect, useSyncExternalStore } from "react";
import * as dls from "../lib/dl";

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

/**
 * Global downloads drawer. Mounted once at App level so transfers kicked off
 * in MARKET or BROWSE stay visible (and cancellable) from every view.
 */
export default function Downloads({ onChanged }: { onChanged?: () => void }) {
  const items = useSyncExternalStore(dls.subscribe, dls.getSnapshot);

  useEffect(() => {
    dls.init();
  }, []);

  // Rescan the index (debounced) after any file lands so new models appear.
  useEffect(() => {
    if (!onChanged) return;
    let t: ReturnType<typeof setTimeout> | null = null;
    const off = dls.onDone((path) => {
      // Emit scan_done event immediately so the UI shows "scanning..." feedback
      import("@tauri-apps/api/event").then(({ emit }) => {
        void emit("scan_started", {});
      });
      if (t) clearTimeout(t);
      t = setTimeout(onChanged, 1500);
    });
    return () => {
      off();
      if (t) clearTimeout(t);
    };
  }, [onChanged]);

  if (items.length === 0) return null;

  const activeCount = items.filter((d) => d.status === "active").length;

  return (
    <div className="dl-drawer">
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 6 }}>
        <span className="mono" style={{ fontSize: 10, letterSpacing: 1 }}>
          DOWNLOADS{activeCount > 0 ? ` · ${activeCount} ACTIVE` : ""}
        </span>
      </div>
      {items.map((d) => {
        const pct = d.total > 0 ? Math.min(100, (d.done / d.total) * 100) : null;
        return (
          <div key={d.key} className={`dl-row ${d.status}`}>
            <div className="row" style={{ gap: 8, alignItems: "center" }}>
              <span className="dl-name" title={d.key}>
                {d.name.split("/").pop()}
              </span>
              {d.status === "active" && (
                <button className="ghost" style={{ fontSize: 9, padding: "2px 7px" }} onClick={() => dls.cancel(d.key)}>
                  ✕
                </button>
              )}
              {d.status === "error" && (
                <span style={{ display: "flex", gap: 6, alignItems: "center" }}>
                  <span className="mono" style={{ fontSize: 9, color: "var(--oom)" }}>FAILED</span>
                  <button
                    className="ghost"
                    style={{ fontSize: 9, padding: "2px 7px" }}
                    title={d.err}
                    onClick={() => dls.removeEntry(d.key)}
                  >
                    ✕
                  </button>
                </span>
              )}
              {d.status === "done" && (
                <span className="mono" style={{ fontSize: 9, color: "var(--pass)" }}>✓ SAVED</span>
              )}
            </div>
            <div className={`dl-bar ${pct == null ? "indeterminate" : ""}`}>
              {pct != null && (
                <div className="dl-fill" style={{ width: `${pct}%` }} />
              )}
            </div>
            <div className="row" style={{ justifyContent: "space-between", marginTop: 3 }}>
              <span className="dl-meta">
                {d.status === "active"
                  ? pct != null
                    ? `${pct.toFixed(1)}% · ${fmtBytes(d.done)} / ${fmtBytes(d.total)}`
                    : fmtBytes(d.done)
                  : d.status === "done"
                    ? fmtBytes(d.total || d.done)
                    : (d.err ?? "failed")}
              </span>
              <span className="dl-meta">{d.status === "active" && ` ${fmtSpeed(d.speed)}`}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}
