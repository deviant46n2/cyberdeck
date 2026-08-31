import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../api";
import * as dls from "../lib/dl";
import { shardSet } from "../lib/shards";
import { AUX_GGUF, verdictClass } from "../lib/ui";

function gib(size: number | null): string {
  if (size == null) return "?";
  return (size / 1_073_741_824).toFixed(2) + " GiB";
}

/** Smallest *model-weight* GGUF file in a repo's file list — the cheapest
 * quant you could actually pull to disk; drives the DISK column.
 * Auxiliary files (mmproj, imatrix, etc.) are excluded. */
function smallestGguf(files: api.MarketFileRow[] | undefined): number | null {
  if (!files) return null;
  const gguf = files.filter(
    (f) =>
      f.rfilename.toLowerCase().endsWith(".gguf") &&
      f.size &&
      !AUX_GGUF.test(f.rfilename),
  );
  if (gguf.length === 0) return null;
  return Math.min(...gguf.map((f) => f.size as number));
}

type SortKey = "downloads" | "likes" | "vram";

export default function Market() {
  // --- hardware ---
  const [hw, setHw] = useState<api.HwInfo | null>(null);

  // --- sources ---
  const [orgs, setOrgs] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [activeOrg, setActiveOrg] = useState<string | null>(null);

  // --- results ---
  const [results, setResults] = useState<api.MarketHit[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [files, setFiles] = useState<Record<string, api.MarketFileRow[]>>({});
  const [fits, setFits] = useState<
    Record<string, Record<string, api.BrowseFitResult>>
  >({});
  /** Smallest-GGUF bytes per repo — the "size on disk" column. */
  const [sizes, setSizes] = useState<Record<string, number | null>>({});

  // --- list-level fit (smallest quant per model, prefetched in background) ---
  const [listFits, setListFits] = useState<Record<string, api.BrowseFitResult>>({});
  const [prefetchDone, setPrefetchDone] = useState(0);
  const prefetchAbort = useRef(0);

  // --- fit params ---
  const [ctx, setCtx] = useState(32768);
  const [ngl, setNgl] = useState(0);
  const [offload, setOffload] = useState(false);
  const [kvBytes, setKvBytes] = useState(0.5);
  const [reserve, setReserve] = useState(1600);

  // --- filters ---
  const [minGiB, setMinGiB] = useState("");
  const [maxGiB, setMaxGiB] = useState("");
  const [sortBy, setSortBy] = useState<SortKey>("downloads");

  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");

  useEffect(() => {
    api.hwInfo().then(setHw);
    api.watchlist().then(setOrgs);
  }, []);

  const PREFETCH_LIMIT = 5;
  const PREFETCH_DELAY_MS = 400;

  /** Fetch the smallest quant's fit + disk size for top results (background,
   * rate-limited). The file list is fetched once and feeds both columns. */
  const prefetchFits = async (hits: api.MarketHit[], gen: number) => {
    const batch = hits.slice(0, PREFETCH_LIMIT);
    setPrefetchDone(0);
    for (let i = 0; i < batch.length; i++) {
      if (gen !== prefetchAbort.current) return;
      const h = batch[i];
      try {
        const ff = await api.marketFiles(h.id);
        if (gen !== prefetchAbort.current) return;
        setSizes((prev) => ({ ...prev, [h.id]: smallestGguf(ff) }));
        const gguf = ff
          .filter(
            (f) =>
              f.rfilename.toLowerCase().endsWith(".gguf") &&
              f.size &&
              !AUX_GGUF.test(f.rfilename),
          )
          .sort((a, b) => (a.size ?? Infinity) - (b.size ?? Infinity));
        if (gguf.length === 0) continue;
        const smallest = gguf[0];
        const r = await api.browseFitRemote({
          repoId: h.id,
          rfilename: smallest.rfilename,
          ctx,
          kvBytes,
          nGpuLayers: ngl,
          kvLayers: null,
          reserve,
          offload,
        });
        if (gen !== prefetchAbort.current) return;
        setListFits((prev) => ({ ...prev, [h.id]: r }));
      } catch {
        // network errors during prefetch are non-fatal
      }
      setPrefetchDone(i + 1);
      if (i < batch.length - 1) {
        await new Promise((r) => setTimeout(r, PREFETCH_DELAY_MS));
      }
    }
  };

  const search = async () => {
    if (!query && !activeOrg) return;
    setBusy(true);
    setMsg("");
    setExpanded(null);
    setListFits({});
    const gen = ++prefetchAbort.current;
    try {
      const hits = activeOrg
        ? await api.browseOrg(activeOrg, 30)
        : await api.marketSearch(query, 30);
      setResults(hits);
      setMsg(hits.length === 0 ? "no results" : `${hits.length} model(s)`);
      setBusy(false);
      // kick off background fit prefetch
      prefetchFits(hits, gen);
    } catch (e) {
      setMsg(`search failed: ${String(e)}`);
      setBusy(false);
    }
  };

  const browseOrg = (org: string) => {
    if (activeOrg === org) {
      setActiveOrg(null);
      return;
    }
    setActiveOrg(org);
    setQuery("");
    const gen = ++prefetchAbort.current;
    api.browseOrg(org, 30).then((hits) => {
      setResults(hits);
      setExpanded(null);
      setListFits({});
      setMsg(hits.length === 0 ? "no results" : `${hits.length} model(s)`);
      prefetchFits(hits, gen);
    });
  };

  const openModel = async (id: string) => {
    if (expanded === id) {
      setExpanded(null);
      return;
    }
    setExpanded(id);
    if (!files[id]) {
      try {
        const f = await api.marketFiles(id);
        setFiles((prev) => ({ ...prev, [id]: f }));
        setSizes((prev) => (prev[id] != null ? prev : { ...prev, [id]: smallestGguf(f) }));
      } catch (e) {
        setMsg(`file list failed: ${String(e)}`);
      }
    }
  };

  const fitFile = async (repoId: string, rfilename: string) => {
    setMsg(`fitting ${rfilename}…`);
    try {
      const r = await api.browseFitRemote({
        repoId,
        rfilename,
        ctx,
        kvBytes,
        nGpuLayers: ngl,
        kvLayers: null,
        reserve,
        offload,
      });
      setFits((prev) => ({
        ...prev,
        [repoId]: { ...(prev[repoId] || {}), [rfilename]: r },
      }));
      // also update list fit if this is the first fit for this model
      setListFits((prev) => {
        if (!prev[repoId]) return { ...prev, [repoId]: r };
        return prev;
      });
      setMsg("");
    } catch (e) {
      setMsg(`fit failed: ${String(e)}`);
    }
  };

  const fitAll = async (repoId: string) => {
    const ff = files[repoId];
    if (!ff) return;
    setMsg(`fitting ${ff.length} file(s)…`);
    for (const f of ff) {
      if (f.rfilename.toLowerCase().endsWith(".gguf")) {
        await fitFile(repoId, f.rfilename);
      }
    }
  };

  /** Queue a download; auto-detects split-GGUF/safetensors shard sets. */
  const startDl = (repoId: string, rfilename: string) => {
    const allNames = (files[repoId] ?? []).map((f) => f.rfilename);
    const parts = shardSet(rfilename, allNames);
    if (parts.length > 1) {
      setMsg(`queued ${parts.length}-part set of ${rfilename} — watch DOWNLOADS`);
      void dls.enqueueSequence(repoId, parts);
    } else {
      dls.enqueue(repoId, rfilename);
      setMsg(`queued ${rfilename} — watch DOWNLOADS`);
    }
  };

  const filteredFiles = useMemo(() => {
    if (!expanded || !files[expanded]) return [];
    let list = files[expanded];
    const min = minGiB ? parseFloat(minGiB) * 1_073_741_824 : 0;
    const max = maxGiB ? parseFloat(maxGiB) * 1_073_741_824 : Infinity;
    list = list.filter((f) => {
      if (!f.size) return true;
      return f.size >= min && f.size <= max;
    });
    list.sort((a, b) => (a.size ?? 0) - (b.size ?? 0));
    return list;
  }, [expanded, files, minGiB, maxGiB]);

  const sortedResults = useMemo(() => {
    const copy = [...results];
    if (sortBy === "downloads") copy.sort((a, b) => b.downloads - a.downloads);
    else if (sortBy === "likes") copy.sort((a, b) => b.likes - a.likes);
    else if (sortBy === "vram") {
      copy.sort((a, b) => {
        const fa = listFits[a.id]?.model_vram_mb ?? Infinity;
        const fb = listFits[b.id]?.model_vram_mb ?? Infinity;
        return fa - fb; // smallest VRAM first
      });
    }
    return copy;
  }, [results, sortBy, listFits]);

  const listFitCount = Object.keys(listFits).length;

  return (
    <>
      <div className="view-title">MARKET</div>

      {/* --- hardware + fit params --- */}
      {hw && (
        <div className="card" style={{ marginBottom: 16 }}>
          <div className="row" style={{ justifyContent: "space-between", flexWrap: "wrap", gap: 12 }}>
            <div>
              <h3>HARDWARE</h3>
              <span className="mono" style={{ fontSize: 12 }}>
                {hw.vram_mb != null
                  ? `GPU VRAM: ${hw.vram_mb.toLocaleString()} MiB`
                  : "GPU VRAM: unknown (no nvidia-smi)"}
              </span>
            </div>
            <div style={{ display: "flex", gap: 20, flexWrap: "wrap" }}>
              <div style={{ minWidth: 160 }}>
                <label className="field">
                  CONTEXT — {ctx.toLocaleString()}
                </label>
                <input
                  type="range"
                  min={2048}
                  max={131072}
                  step={2048}
                  value={ctx}
                  onChange={(e) => setCtx(parseInt(e.target.value))}
                />
              </div>
              <div style={{ minWidth: 140 }}>
                <label className="field">
                  GPU LAYERS — {ngl === 0 ? "all" : ngl}
                </label>
                <input
                  type="range"
                  min={0}
                  max={128}
                  step={1}
                  value={ngl}
                  onChange={(e) => setNgl(parseInt(e.target.value))}
                />
              </div>
              <div style={{ minWidth: 100 }}>
                <label className="field">KV BYTES</label>
                <select
                  value={kvBytes}
                  onChange={(e) => setKvBytes(parseFloat(e.target.value))}
                >
                  <option value={0.5}>q4 (0.5)</option>
                  <option value={1.0}>q8 (1.0)</option>
                  <option value={2.0}>fp16 (2.0)</option>
                </select>
              </div>
              <div style={{ minWidth: 100 }}>
                <label className="field">RESERVE (MiB)</label>
                <input
                  type="number"
                  value={reserve}
                  onChange={(e) => setReserve(parseInt(e.target.value) || 1600)}
                />
              </div>
              <label className="row" style={{ gap: 6, fontSize: 12, alignSelf: "flex-end", paddingBottom: 4 }}>
                <input
                  type="checkbox"
                  checked={offload}
                  onChange={(e) => setOffload(e.target.checked)}
                />
                FreeToken offload
              </label>
            </div>
          </div>
        </div>
      )}

      {/* --- search + filters --- */}
      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row">
          <div style={{ flex: 1, minWidth: 240 }}>
            <input
              type="text"
              placeholder="search HuggingFace (e.g. Qwen3.8 GGUF)"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setActiveOrg(null);
              }}
              onKeyDown={(e) => e.key === "Enter" && search()}
            />
          </div>
          <button className="action" onClick={search} disabled={busy}>
            SEARCH
          </button>
        </div>
        {orgs.length > 0 && (
          <div className="row" style={{ marginTop: 10, flexWrap: "wrap", gap: 6 }}>
            {orgs.map((o) => (
              <span
                key={o}
                className={`badge ${activeOrg === o ? "mag" : ""}`}
                style={{ cursor: "pointer" }}
                onClick={() => browseOrg(o)}
              >
                {o}
              </span>
            ))}
          </div>
        )}
        <div className="row" style={{ marginTop: 10, gap: 12, flexWrap: "wrap" }}>
          <div>
            <label className="field">MIN GiB</label>
            <input
              type="number"
              placeholder="0"
              value={minGiB}
              onChange={(e) => setMinGiB(e.target.value)}
              style={{ width: 80 }}
            />
          </div>
          <div>
            <label className="field">MAX GiB</label>
            <input
              type="number"
              placeholder="∞"
              value={maxGiB}
              onChange={(e) => setMaxGiB(e.target.value)}
              style={{ width: 80 }}
            />
          </div>
          <div>
            <label className="field">SORT</label>
            <select value={sortBy} onChange={(e) => setSortBy(e.target.value as SortKey)}>
              <option value="downloads">downloads</option>
              <option value="likes">likes</option>
              <option value="vram">VRAM (ascending)</option>
            </select>
          </div>
        </div>
        {msg && (
          <div className="dim" style={{ marginTop: 10, fontSize: 11 }}>
            {msg}
          </div>
        )}
      </div>

      {/* --- results --- */}
      {sortedResults.length > 0 && (
        <div className="card">
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
            <h3 style={{ margin: 0 }}>RESULTS</h3>
            {listFitCount < sortedResults.length && listFitCount < PREFETCH_LIMIT && (
              <span className="dim" style={{ fontSize: 11 }}>
                estimating fit… {Math.min(listFitCount, PREFETCH_LIMIT)}/{Math.min(sortedResults.length, PREFETCH_LIMIT)}
              </span>
            )}
          </div>
          <table>
            <thead>
              <tr>
                <th></th>
                <th>REPO</th>
                <th>DISK</th>
                <th>VRAM</th>
                <th>TASK</th>
                <th>⬇</th>
                <th>♥</th>
              </tr>
            </thead>
            <tbody>
              {sortedResults.map((h) => {
                const lf = listFits[h.id];
                const size = sizes[h.id];
                return (
                  <Fragment key={h.id}>
                    <tr
                      onClick={() => openModel(h.id)}
                      style={{ cursor: "pointer" }}
                    >
                      <td className="dim">{expanded === h.id ? "▾" : "▸"}</td>
                      <td className="mono">{h.id}</td>
                      <td className="mono" style={{ fontSize: 11 }}>
                        {size != null ? gib(size) : <span className="dim" style={{ fontSize: 10 }}>…</span>}
                      </td>
                      <td>
                        {lf ? (
                          <span className="row" style={{ gap: 6, alignItems: "center" }}>
                            <span className={`badge ${verdictClass(lf.verdict)}`} style={{ fontSize: 10 }}>
                              {lf.verdict}
                            </span>
                            <span className="mono" style={{ fontSize: 11 }}>
                              {lf.model_vram_mb.toLocaleString()} MiB
                            </span>
                          </span>
                        ) : (
                          <span className="dim" style={{ fontSize: 10 }}>…</span>
                        )}
                      </td>
                      <td>{h.pipeline_tag ?? "—"}</td>
                      <td className="mono">{h.downloads.toLocaleString()}</td>
                      <td className="mono">{h.likes}</td>
                    </tr>
                    {expanded === h.id && (
                      <tr key={h.id + "/files"}>
                        <td></td>
                        <td colSpan={6}>
                          <div style={{ padding: "8px 0" }}>
                            <div className="row" style={{ gap: 8, marginBottom: 8 }}>
                              <button
                                className="action"
                                onClick={() => fitAll(h.id)}
                                disabled={busy}
                                style={{ fontSize: 11 }}
                              >
                                FIT ALL
                              </button>
                              <span className="dim" style={{ fontSize: 10 }}>
                                {filteredFiles.length} file(s)
                              </span>
                            </div>
                            {!files[h.id] && (
                              <span className="dim">loading files…</span>
                            )}
                            {filteredFiles.map((f) => {
                              const fitResult = fits[h.id]?.[f.rfilename];
                              return (
                                <div
                                  key={f.rfilename}
                                  className="row"
                                  style={{
                                    justifyContent: "space-between",
                                    padding: "4px 0",
                                    gap: 8,
                                  }}
                                >
                                  <span className="mono" style={{ fontSize: 11 }}>
                                    {f.rfilename} · {gib(f.size)}
                                  </span>
                                  <div className="row" style={{ gap: 6 }}>
                                    {fitResult && (
                                      <span
                                        className={`badge ${verdictClass(fitResult.verdict)}`}
                                      >
                                        {fitResult.verdict}
                                      </span>
                                    )}
                                    {fitResult && (
                                      <span className="dim" style={{ fontSize: 10 }}>
                                        {fitResult.model_vram_mb.toLocaleString()} MiB
                                        {fitResult.arch && ` · ${fitResult.arch}`}
                                        {fitResult.n_layers && ` · ${fitResult.n_layers}L`}
                                        {fitResult.params &&
                                          ` · ${(fitResult.params / 1e9).toFixed(1)}B`}
                                      </span>
                                    )}
                                    <button
                                      className="ghost"
                                      onClick={() => fitFile(h.id, f.rfilename)}
                                    >
                                      FIT
                                    </button>
                                    <button
                                      className="ghost"
                                      onClick={() => startDl(h.id, f.rfilename)}
                                    >
                                      DOWNLOAD
                                    </button>
                                  </div>
                                </div>
                              );
                            })}
                          </div>
                        </td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
