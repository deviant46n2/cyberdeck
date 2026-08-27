import { useState } from "react";
import * as api from "../api";
import * as br from "../lib/br";

interface VaultProps {
  models: api.ModelRow[];
  dups: api.DupRow[];
  onRefresh: () => void;
  onReload: () => void;
}

export default function Vault({ models, dups, onRefresh, onReload }: VaultProps) {
  const [deleting, setDeleting] = useState<Set<string>>(new Set());
  const [flash, setFlash] = useState<string | null>(null);
  const dupIds = new Set(dups.flatMap((d) => d.members));

  const load = (path: string, engine: "llamacpp" | "freetoken") => {
    if (!confirm(`LOAD ${engine} — derive max-ctx, verify on test port, then go live?\n${path}`)) {
      return;
    }
    void br.startBringup(path, engine);
  };

  const remove = async (path: string) => {
    if (!confirm(`Delete "${path}"\n\nThis removes the index entry and deletes the file from disk.`)) return;
    setDeleting((prev) => new Set(prev).add(path));
    try {
      const deleted = await api.deleteModel(path, true);
      if (deleted > 0) {
        setFlash(path);
        setTimeout(() => setFlash(null), 2000);
        void onReload();
      }
    } catch (e) {
      alert(`Delete failed: ${String(e)}`);
    } finally {
      setDeleting((prev) => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
    }
  };

  return (
    <>
      <div className="view-title">VAULT</div>

      {dups.length > 0 && (
        <div className="card" style={{ marginBottom: 16, borderColor: "var(--oom)" }}>
          <h3 style={{ color: "var(--oom)" }}>DUPLICATE SHARDS — WASTED SPACE</h3>
          {dups.map((d) => (
            <div key={d.identity} style={{ margin: "8px 0" }}>
              <span className="badge oom">{d.wasted_gib.toFixed(2)} GiB</span>{" "}
              <span className="mono">{d.identity}</span>
              <ul className="dim" style={{ margin: "4px 0 0 18px", fontSize: 11 }}>
                {d.members.map((m) => (
                  <li key={m}>{m}</li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}

      <div className="card">
        <table>
          <thead>
            <tr>
              <th>NAME</th>
              <th>QUANT</th>
              <th>ARCH</th>
              <th>TRAIN CTX</th>
              <th>SIZE</th>
              <th>PATH</th>
              <th>LOAD</th>
              <th>DELETE</th>
            </tr>
          </thead>
          <tbody>
            {models.length === 0 ? (
              <tr>
                <td colSpan={8} className="dim">
                  no models indexed — run a SCAN from HUD
                </td>
              </tr>
            ) : (
              models
                .filter((m) => flash !== m.path)
                .map((m) => {
                  const dup = dupIds.has(m.path);
                  const isDeleting = deleting.has(m.path);
                  return (
                    <tr
                      key={m.path}
                      style={{
                        ...((dup || flash === m.path) ? { background: "rgba(255,59,59,0.06)" } : undefined),
                        opacity: isDeleting ? 0.3 : 1,
                        transition: "opacity 0.2s",
                      }}
                    >
                    <td>{m.name}</td>
                    <td>{m.quant ?? "—"}</td>
                    <td>{m.arch ?? "—"}</td>
                    <td className="mono">{m.ctx_train ? m.ctx_train.toLocaleString() : "—"}</td>
                    <td className="mono">{m.footprint_gib.toFixed(2)} GiB</td>
                    <td className="dim" style={{ maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {m.path}
                    </td>
                    <td>
                      <div style={{ display: "flex", gap: 4 }}>
                        <button
                          className="ghost"
                          style={{ fontSize: 9, padding: "3px 7px" }}
                          title="bring up via llama.cpp — derive max-ctx → verify on test port → live"
                          onClick={() => load(m.path, "llamacpp")}
                          disabled={m.path.toLowerCase().endsWith(".safetensors")}
                        >
                          LCPP
                        </button>
                        <button
                          className="ghost"
                          style={{ fontSize: 9, padding: "3px 7px" }}
                          title="bring up via FreeToken offload — derive max-ctx → verify on test port → live"
                          onClick={() => load(m.path, "freetoken")}
                        >
                          FT
                        </button>
                      </div>
                    </td>
                    <td>
                      <button
                        className="ghost"
                        style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--oom)", color: "var(--oom)" }}
                        onClick={() => remove(m.path)}
                        disabled={isDeleting}
                        title={isDeleting ? "deleting..." : "delete from index and disk"}
                      >
                        {isDeleting ? "..." : "✕"}
                      </button>
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}
