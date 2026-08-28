import { Fragment, useState } from "react";
import * as api from "../api";
import * as dls from "../lib/dl";
import { shardSet } from "../lib/shards";

function gib(size: number | null): string {
  if (size == null) return "?";
  return (size as number / 1_073_741_824).toFixed(2) + " GiB";
}

export default function Market() {
  const [query, setQuery] = useState("Qwen3.8 GGUF");
  const [hits, setHits] = useState<api.MarketHit[] | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [files, setFiles] = useState<api.MarketFileRow[]>([]);
  const [msg, setMsg] = useState<string>("");
  const [busy, setBusy] = useState(false);

  const search = async () => {
    if (!query) return;
    setBusy(true);
    setMsg("");
    try {
      setHits(await api.marketSearch(query, 20));
    } catch (e) {
      setMsg(`search failed: ${String(e)}`);
    }
    setBusy(false);
  };

  const open = async (id: string) => {
    if (expanded === id) {
      setExpanded(null);
      return;
    }
    setExpanded(id);
    setFiles([]);
    try {
      setFiles(await api.marketFiles(id));
    } catch (e) {
      setMsg(`file list failed: ${String(e)}`);
    }
  };

  /** Queue a download; auto-detects split-GGUF/safetensors shard sets. */
  const startDl = (repoId: string, rfilename: string, allNames: string[]) => {
    const parts = shardSet(rfilename, allNames);
    if (parts.length > 1) {
      setMsg(`queued ${parts.length}-part set of ${rfilename} — watch DOWNLOADS`);
      void dls.enqueueSequence(repoId, parts);
    } else {
      dls.enqueue(repoId, rfilename);
      setMsg(`queued ${rfilename} — watch DOWNLOADS`);
    }
  };

  return (
    <>
      <div className="view-title">MARKET</div>

      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row">
          <div style={{ flex: 1, minWidth: 240 }}>
            <input
              type="text"
              placeholder="search HuggingFace (e.g. Qwen3.8 GGUF)"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && search()}
            />
          </div>
          <button className="action" onClick={search} disabled={busy}>
            SEARCH
          </button>
        </div>
        {msg && (
          <div className="dim" style={{ marginTop: 10, fontSize: 11 }}>
            {msg}
          </div>
        )}
      </div>

      {hits && hits.length === 0 && (
        <div className="card">
          <div className="dim" style={{ fontSize: 11 }}>
            no results — try a broader query
          </div>
        </div>
      )}

      {hits && hits.length > 0 && (
        <div className="card">
          <h3>RESULTS</h3>
          <table>
            <thead>
              <tr>
                <th></th>
                <th>REPO</th>
                <th>TASK</th>
                <th>⬇</th>
                <th>♥</th>
              </tr>
            </thead>
            <tbody>
              {hits.map((h) => (
                <Fragment key={h.id}>
                  <tr
                    onClick={() => open(h.id)}
                    style={{ cursor: "pointer" }}
                  >
                    <td className="dim">{expanded === h.id ? "▾" : "▸"}</td>
                    <td className="mono">{h.id}</td>
                    <td>{h.pipeline_tag ?? "—"}</td>
                    <td className="mono">{h.downloads.toLocaleString()}</td>
                    <td className="mono">{h.likes}</td>
                  </tr>
                  {expanded === h.id && (
                    <tr key={h.id + "/files"}>
                      <td></td>
                      <td colSpan={4}>
                        <div style={{ padding: "8px 0" }}>
                          {files.length === 0 && (
                            <span className="dim">loading files…</span>
                          )}
                          {files.map((f) => (
                            <div
                              key={f.rfilename}
                              className="row"
                              style={{ justifyContent: "space-between", padding: "4px 0" }}
                            >
                              <span className="mono" style={{ fontSize: 11 }}>
                                {f.rfilename} · {gib(f.size)}
                              </span>
                              <button
                                className="ghost"
                                onClick={() => startDl(h.id, f.rfilename, files.map((x) => x.rfilename))}
                              >
                                DOWNLOAD
                              </button>
                            </div>
                          ))}
                        </div>
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
