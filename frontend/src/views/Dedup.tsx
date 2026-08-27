import { useState } from "react";
import * as api from "../api";

interface DedupProps {
  dups: api.DupRow[];
  onRefresh: () => void;
}

export default function Dedup({ dups, onRefresh }: DedupProps) {
  const [deleting, setDeleting] = useState<Set<string>>(new Set());

  if (dups.length === 0) {
    return (
      <>
        <div className="view-title">DEDUP</div>
        <div className="card">
          <div className="dim" style={{ fontSize: 12, textAlign: "center", padding: 24 }}>
            no duplicate models found
          </div>
        </div>
      </>
    );
  }

  const totalWasted = dups.reduce((sum, d) => sum + d.wasted_gib, 0);

  return (
    <>
      <div className="view-title">DEDUP</div>

      <div className="card" style={{ marginBottom: 16, borderColor: "var(--oom)" }}>
        <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ color: "var(--oom)", margin: 0 }}>
            {dups.length} duplicate group{dups.length !== 1 ? "s" : ""} · {totalWasted.toFixed(2)} GiB wasted
          </h3>
        </div>
      </div>

      {dups.map((d) => (
        <div key={d.identity} className="card" style={{ marginBottom: 12, borderColor: "var(--oom)" }}>
          <div className="row" style={{ justifyContent: "space-between", marginBottom: 8 }}>
            <div>
              <span className="badge oom">{d.wasted_gib.toFixed(2)} GiB wasted</span>{" "}
              <span className="mono" style={{ fontSize: 11, marginLeft: 8 }}>
                {d.identity}
              </span>
            </div>
            <button
              className="ghost"
              style={{ fontSize: 9, padding: "3px 8px", borderColor: "var(--oom)", color: "var(--oom)" }}
              disabled={deleting.has(d.identity)}
              onClick={() => handleDedupDelete(d.identity, deleting, setDeleting, onRefresh)}
            >
              {deleting.has(d.identity) ? "..." : "DELETE"}
            </button>
          </div>
          <ul className="dim" style={{ margin: 0, padding: "0 0 0 18px", fontSize: 11 }}>
            {d.members.map((m, i) => (
              <li key={i}>
                {m}
              </li>
            ))}
          </ul>
        </div>
      ))}

      <button
        className="ghost"
        style={{ marginTop: 8, fontSize: 10, padding: "6px 12px", borderColor: "var(--oom)", color: "var(--oom)" }}
        disabled={deleting.size > 0}
        onClick={() => handleDeleteAll(dups, deleting, setDeleting, onRefresh)}
      >
        DELETE ALL DUPLICATES ({dups.reduce((s, d) => s + d.members.length - 1, 0)} files, ~{totalWasted.toFixed(2)} GiB)
      </button>
    </>
  );
}

async function handleDedupDelete(
  identity: string,
  deleting: Set<string>,
  setDeleting: React.Dispatch<React.SetStateAction<Set<string>>>,
  onRefresh: () => void
) {
  if (!confirm(`Delete all duplicate copies of "${identity}" except the cheapest one?`)) return;
  setDeleting((prev) => new Set(prev).add(identity));
  try {
    const deleted = await api.dedupDelete(identity, true);
    if (deleted > 0) {
      void onRefresh();
    }
  } catch (e) {
    alert(`Delete failed: ${String(e)}`);
  } finally {
    setDeleting((prev) => {
      const next = new Set(prev);
      next.delete(identity);
      return next;
    });
  }
}

async function handleDeleteAll(
  dups: api.DupRow[],
  deleting: Set<string>,
  setDeleting: React.Dispatch<React.SetStateAction<Set<string>>>,
  onRefresh: () => void
) {
  if (!confirm("Delete ALL duplicate models (all but cheapest copy) across all groups?")) return;
  setDeleting((prev) => new Set(prev));
  try {
    let total = 0;
    for (const d of dups) {
      total += await api.dedupDelete(d.identity, true);
    }
    if (total > 0) {
      void onRefresh();
    }
  } catch (e) {
    alert(`Delete failed: ${String(e)}`);
  } finally {
    setDeleting((prev) => new Set());
  }
}
