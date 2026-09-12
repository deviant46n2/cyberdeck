import { useState } from "react";
import * as api from "../api";

// What is consuming disk, who owns it, and what cleanup would delete — the
// filesystem-as-truth view. Missing rows (DB claims, file gone) can be
// forgotten; orphaned files (on disk, unregistered) can be deleted with exact
// byte accounting. Re-indexing orphans is a rescan away.
export default function StoragePanel({ onChanged }: { onChanged: () => void }) {
  const [open, setOpen] = useState(false);
  const [rep, setRep] = useState<api.StorageReport | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    setBusy(true);
    setErr("");
    try {
      setRep(await api.storageReconcile());
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && !rep) void refresh();
  };

  const forgetMissing = async (path: string) => {
    if (!confirm(`Forget missing record?\n\n${path}\n\nThe file is already gone; this clears the phantom DB row.`)) return;
    try {
      await api.forgetModel(path);
      void refresh();
      onChanged();
    } catch (e) {
      alert(String(e));
    }
  };

  const deleteOrphan = async (path: string, bytes: number) => {
    if (!confirm(`Delete orphaned file?\n\n${path}\n${(bytes / 1073741824).toFixed(2)} GiB will be freed.\n\nNo library record references it.`)) return;
    try {
      await api.removeModelFiles([path]);
      void refresh();
      onChanged();
    } catch (e) {
      alert(String(e));
    }
  };

  const gib = (b: number) => `${(b / 1073741824).toFixed(2)} GiB`;

  return (
    <div style={{ border: "1px solid var(--dim)", borderRadius: 6, marginBottom: 10 }}>
      <button
        className="ghost"
        style={{ width: "100%", justifyContent: "flex-start", fontSize: 11, padding: "6px 10px" }}
        onClick={toggle}
      >
        {open ? "▾" : "▸"} STORAGE
        {rep && (
          <span className="dim" style={{ marginLeft: 8 }}>
            {rep.total_gib.toFixed(1)} GiB on disk · {rep.ok_count} healthy ·{" "}
            {rep.missing.length} missing · {rep.orphaned.length} orphaned
          </span>
        )}
        {busy && <span className="dim" style={{ marginLeft: 8 }}>…</span>}
      </button>
      {open && (
        <div style={{ padding: "4px 10px 10px", fontSize: 10 }}>
          {err && <div style={{ color: "var(--oom)" }}>{err}</div>}
          {rep ? (
            <>
              <div className="dim" style={{ marginBottom: 6 }}>
                {rep.ok_gib.toFixed(1)} GiB registered & present ·{" "}
                <button className="ghost" style={{ fontSize: 9, padding: "2px 6px" }} onClick={() => refresh()} disabled={busy}>
                  refresh
                </button>
              </div>
              {rep.missing.length > 0 && (
                <div style={{ marginBottom: 8 }}>
                  <div style={{ color: "var(--warn)", marginBottom: 4 }}>
                    MISSING — database claims these, files are gone:
                  </div>
                  {rep.missing.map((m) => (
                    <div key={m.path} style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 2 }}>
                      <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={m.path}>
                        {m.name} <span className="dim">({gib(m.claimed_bytes)} claimed)</span>
                      </span>
                      <button className="ghost" style={{ fontSize: 9, padding: "2px 6px" }} onClick={() => forgetMissing(m.path)}>
                        forget
                      </button>
                    </div>
                  ))}
                </div>
              )}
              {rep.orphaned.length > 0 && (
                <div>
                  <div style={{ color: "var(--cyan)", marginBottom: 4 }}>
                    ORPHANED — on disk, no library record (rescan re-indexes, or delete):
                  </div>
                  {rep.orphaned.map((o) => (
                    <div key={o.path} style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 2 }}>
                      <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={o.path}>
                        {o.name} <span className="dim">({gib(o.bytes)})</span>
                      </span>
                      <button
                        className="ghost"
                        style={{ fontSize: 9, padding: "2px 6px", borderColor: "var(--oom)", color: "var(--oom)" }}
                        onClick={() => deleteOrphan(o.path, o.bytes)}
                      >
                        delete
                      </button>
                    </div>
                  ))}
                </div>
              )}
              {rep.missing.length === 0 && rep.orphaned.length === 0 && (
                <div className="dim">Clean — every record has its file, every file has its record.</div>
              )}
            </>
          ) : (
            !err && <div className="dim">Loading…</div>
          )}
        </div>
      )}
    </div>
  );
}
