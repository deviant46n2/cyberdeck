import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as api from "../api";
import * as dls from "../lib/dl";
import { shardSet } from "../lib/shards";
import { AUX_GGUF } from "../lib/ui";

const SCORE_COLOR = (s: number) => (s >= 0.65 ? "var(--pass)" : s >= 0.4 ? "var(--warn)" : "var(--dim2)");
const RESERVE_MB = 1600;
const KV_FLOOR_MB = 1024;
const PAGE = 20;

type SR = api.ScoredRelease;

const QUANT_ORDER = ["Q4_K_M", "Q5_K_M", "Q8_0", "Q4_0", "Q5_0", "Q6_K", "IQ4_XS", "F16"] as const;
type QuantToken = (typeof QUANT_ORDER)[number];
const QUANT_ABBR: Record<QuantToken, string> = {
  Q4_K_M: "q4km", Q5_K_M: "q5km", Q8_0: "q8", Q4_0: "q4",
  Q5_0: "q5", Q6_K: "q6k", IQ4_XS: "iq4xs", F16: "f16",
};
const QUANT_PATTERN: Record<QuantToken, RegExp> = {
  Q4_K_M: /q4[_\-]?k[_\-]?m/i, Q5_K_M: /q5[_\-]?k[_\-]?m/i,
  Q8_0: /q8[_\-]?0/i, Q4_0: /q4[_\-]?0/i, Q5_0: /q5[_\-]?0/i,
  Q6_K: /q6[_\-]?k/i, IQ4_XS: /iq4[_\-]?xs/i, F16: /\bf16\b|\bfp16\b|\bbf16\b/i,
};

type SortKey = "score" | "fits" | "disk" | "ctx" | "type" | "repo";
type SortDir = "asc" | "desc";

function latestEngines(items: SR[]): SR[] {
  const best = new Map<string, SR>();
  for (const r of items) {
    if (r.release.source !== "github") continue;
    const key = r.release.repo;
    const existing = best.get(key);
    if (!existing || r.release.fetched_at > existing.release.fetched_at) {
      best.set(key, r);
    }
  }
  return [...best.values()].sort((a, b) => b.release.fetched_at - a.release.fetched_at);
}

function autoQuant(quantSizes: [string, number][], vramMb: number): QuantToken {
  const budget = vramMb - RESERVE_MB - KV_FLOOR_MB;
  const fitting = quantSizes
    .filter(([_, gb]) => gb * 1024 < budget)
    .sort((a, b) => b[1] - a[1]);
  return (fitting[0]?.[0] as QuantToken) ?? "Q4_K_M";
}

function sortModels(items: SR[], key: SortKey, dir: SortDir, selMap: Map<string, QuantToken>): SR[] {
  const sorted = [...items];
  const mul = dir === "asc" ? 1 : -1;
  sorted.sort((a, b) => {
    switch (key) {
      case "score": return mul * (a.score.total - b.score.total);
      case "fits": return mul * ((a.score.fits ? 1 : 0) - (b.score.fits ? 1 : 0));
      case "disk": {
        const qa = Object.fromEntries(a.score.quant_sizes);
        const qb = Object.fromEntries(b.score.quant_sizes);
        const da = qa[selMap.get(a.release.repo) ?? "Q4_K_M"] ?? a.score.disk_gb ?? 999;
        const db = qb[selMap.get(b.release.repo) ?? "Q4_K_M"] ?? b.score.disk_gb ?? 999;
        return mul * (da - db);
      }
      case "ctx": return mul * ((a.score.max_ctx ?? 0) - (b.score.max_ctx ?? 0));
      case "type": return mul * a.score.model_type.localeCompare(b.score.model_type);
      case "repo": return mul * a.release.repo.localeCompare(b.release.repo);
      default: return 0;
    }
  });
  return sorted;
}

const COLS: { key: SortKey; label: string; w?: number }[] = [
  { key: "score", label: "SCORE" },
  { key: "fits", label: "FITS" },
  { key: "type", label: "TYPE", w: 50 },
  { key: "disk", label: "DISK" },
  { key: "ctx", label: "MAX CTX" },
  { key: "repo", label: "REPO" },
];

export default function Feeds() {
  const [allRanked, setAllRanked] = useState<SR[] | null>(null);
  const [visible, setVisible] = useState(PAGE);
  const [dlBusy, setDlBusy] = useState<string | null>(null);
  const [dlMsg, setDlMsg] = useState<string>("");
  const [selMap, setSelMap] = useState<Map<string, QuantToken>>(new Map());
  const [sortKey, setSortKey] = useState<SortKey>("score");
  const [sortDir, setSortDir] = useState<SortDir>("desc");
  const vramRef = useRef<number>(16000);

  useEffect(() => {
    api.hwInfo().then((hw) => { vramRef.current = hw?.vram_mb ?? 16000; }).catch(() => {});
  }, []);

  useEffect(() => {
    if (!allRanked) return;
    const vram = vramRef.current;
    setSelMap((prev) => {
      const next = new Map(prev);
      for (const r of allRanked) {
        if (r.release.source !== "hf" || r.release.kind !== "model") continue;
        if (next.has(r.release.repo)) continue;
        next.set(r.release.repo, autoQuant(r.score.quant_sizes, vram));
      }
      return next;
    });
  }, [allRanked]);

  const download = async (repoId: string, quant: QuantToken) => {
    setDlBusy(repoId);
    try {
      const [ff, hw] = await Promise.all([api.marketFiles(repoId), api.hwInfo().catch(() => null)]);
      const pat = QUANT_PATTERN[quant];
      const all = ff.filter((f) => f.rfilename.toLowerCase().endsWith(".gguf") && f.size);
      const matches = all.filter((f) => !AUX_GGUF.test(f.rfilename) && pat.test(f.rfilename));
      if (matches.length === 0) {
        setDlMsg(`${repoId}: no ${quant} GGUF in repo`);
        return;
      }
      const bySizeAsc = [...matches].sort((a, b) => (a.size ?? 0) - (b.size ?? 0));
      const vramMb = hw?.vram_mb ?? 16000;
      const capBytes = (vramMb - RESERVE_MB - KV_FLOOR_MB) * 1024 * 1024;
      const pick = (bySizeAsc.filter((f) => (f.size ?? 0) <= capBytes).pop() ?? bySizeAsc[0]).rfilename;
      const parts = shardSet(pick, ff.map((f) => f.rfilename));
      if (parts.length > 1) {
        void dls.enqueueSequence(repoId, parts);
        setDlMsg(`${repoId}: queued ${parts.length}-part set (${pick}…)`);
      } else {
        dls.enqueue(repoId, pick);
        setDlMsg(`${repoId}: queued ${pick}`);
      }
    } catch (e) {
      setDlMsg(`${repoId}: ${String(e)}`);
    } finally {
      setDlBusy(null);
    }
  };

  const load = useCallback(async () => {
    try { await api.feedsPoll([]); } catch { /* non-fatal */ }
    try {
      const r = await api.feedsRank(200, null);
      setAllRanked(r);
    } catch { /* ignore */ }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDir((d) => (d === "desc" ? "asc" : "desc"));
    } else {
      setSortKey(key);
      setSortDir(key === "repo" ? "asc" : "desc");
    }
  };

  const models = useMemo(() => {
    if (!allRanked) return [];
    return allRanked.filter((r) => r.release.source !== "github");
  }, [allRanked]);

  const sorted = useMemo(() => sortModels(models, sortKey, sortDir, selMap), [models, sortKey, sortDir, selMap]);

  const visibleModels = sorted.slice(0, visible);
  const hasMore = visible < sorted.length;

  const sortIndicator = (key: SortKey) => {
    if (sortKey !== key) return "";
    return sortDir === "desc" ? " ▼" : " ▲";
  };

  if (!allRanked) {
    return (
      <>
        <div className="view-title">FEEDS</div>
        <div className="card" style={{ marginBottom: 12, padding: "8px 12px" }}>
          <div style={{ fontSize: 10, color: "var(--dim2)", marginBottom: 4, letterSpacing: 0.5, textTransform: "uppercase" }}>
            engine updates
          </div>
          <div style={{ display: "flex", gap: 12 }}>
            {[1, 2, 3].map((i) => (
              <div key={i} className="mono dim" style={{ fontSize: 11, width: 60, height: 14, background: "var(--panel-2)", borderRadius: 3, opacity: 0.4 }} />
            ))}
          </div>
        </div>
        <div className="card">
          <table>
            <thead>
              <tr>
                <th>#</th>{COLS.map((c) => <th key={c.key}>{c.label}</th>)}<th>WHY</th><th>QUANT</th>
              </tr>
            </thead>
            <tbody>
              {Array.from({ length: 8 }).map((_, i) => (
                <tr key={`skel-${i}`} style={{ opacity: 0.25 }}>
                  <td className="mono dim">{i + 1}</td>
                  <td className="mono">-.--</td>
                  <td className="mono">…</td>
                  <td className="mono dim" />
                  <td className="mono dim">-G</td>
                  <td className="mono dim">-</td>
                  <td><div style={{ width: 180, height: 12, background: "var(--panel-2)", borderRadius: 3 }} /></td>
                  <td><div style={{ width: 120, height: 10, background: "var(--panel-2)", borderRadius: 3 }} /></td>
                  <td />
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </>
    );
  }

  const engines = latestEngines(allRanked);

  return (
    <>
      <div className="view-title">FEEDS</div>

      {engines.length > 0 && (
        <div className="card" style={{ marginBottom: 12, padding: "8px 12px" }}>
          <div style={{ fontSize: 10, color: "var(--dim2)", marginBottom: 4, letterSpacing: 0.5, textTransform: "uppercase" }}>
            engine updates
          </div>
          <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
            {engines.map((e) => (
              <a
                key={e.release.repo}
                href={e.release.url}
                target="_blank"
                rel="noreferrer"
                style={{ fontSize: 11, color: "var(--cyan)", textDecoration: "none" }}
              >
                {e.release.repo.split("/").pop()}
                <span className="dim" style={{ marginLeft: 4 }}>@{e.release.rev.slice(0, 7)}</span>
              </a>
            ))}
          </div>
        </div>
      )}

      {models.length === 0 && (
        <div className="dim" style={{ padding: 20, fontSize: 12 }}>
          no models ranked — hardware-grounded relevance will appear after a poll
        </div>
      )}

      <div className="card">
        <table>
          <thead>
            <tr>
              <th>#</th>
              {COLS.map((c) => (
                <th
                  key={c.key}
                  onClick={() => toggleSort(c.key)}
                  style={{ cursor: "pointer", userSelect: "none", width: c.w }}
                >
                  {c.label}{sortIndicator(c.key)}
                </th>
              ))}
              <th>WHY</th>
              <th>QUANT</th>
            </tr>
          </thead>
          <tbody>
            {visibleModels.map((r, i) => {
              const sel = selMap.get(r.release.repo) ?? "Q4_K_M";
              const qs = Object.fromEntries(r.score.quant_sizes);
              const diskGb = qs[sel] ?? r.score.disk_gb;

              return (
                <tr key={`${r.release.source}:${r.release.repo}@${r.release.rev}`}>
                  <td className="mono dim">{i + 1}</td>
                  <td className="mono" style={{ color: SCORE_COLOR(r.score.total) }}>
                    {r.score.total.toFixed(2)}
                  </td>
                  <td className="mono" style={{ color: r.score.fits ? "var(--pass)" : "var(--oom)" }}>
                    {r.score.fits ? "✓" : "✗"}
                  </td>
                  <td className="mono dim" style={{ fontSize: 10, color: r.score.model_type === "moe" ? "var(--cyan)" : undefined }}>
                    {r.score.model_type}
                  </td>
                  <td className="mono dim">
                    {diskGb != null ? `~${diskGb.toFixed(1)}G` : "-"}
                  </td>
                  <td className="mono dim">
                    {r.score.max_ctx != null ? `@${r.score.max_ctx}` : "-"}
                  </td>
                  <td className="mono" style={{ maxWidth: 220, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    <a href={r.release.url} target="_blank" rel="noreferrer" style={{ color: "var(--cyan)" }}>
                      {r.release.repo}
                    </a>
                    <span className="dim" style={{ marginLeft: 4 }}>@{r.release.rev.slice(0, 7)}</span>
                  </td>
                  <td className="dim" style={{ fontSize: 10 }}>
                    {r.score.reasons.join(", ")}
                  </td>
                  <td style={{ whiteSpace: "nowrap" }}>
                    {r.release.source === "hf" && r.release.kind === "model" ? (
                      <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                        <select
                          className="ghost mono"
                          style={{ fontSize: 10, padding: "1px 4px", width: 60 }}
                          value={sel}
                          onChange={(e) => setSelMap((prev) => new Map(prev).set(r.release.repo, e.target.value as QuantToken))}
                        >
                          {QUANT_ORDER.map((q) => {
                            const gb = qs[q];
                            return (
                              <option key={q} value={q}>
                                {QUANT_ABBR[q]}{gb != null ? ` ~${gb.toFixed(0)}G` : ""}
                              </option>
                            );
                          })}
                        </select>
                        <button
                          className="ghost"
                          style={{ fontSize: 10, padding: "1px 6px" }}
                          disabled={dlBusy === r.release.repo}
                          onClick={() => void download(r.release.repo, sel)}
                        >
                          {dlBusy === r.release.repo ? "…" : "DL"}
                        </button>
                      </span>
                    ) : (
                      <span className="dim">—</span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
        {hasMore && (
          <button
            className="ghost"
            style={{ display: "block", margin: "8px auto", fontSize: 11, padding: "4px 16px" }}
            onClick={() => setVisible((v) => v + PAGE)}
          >
            show {Math.min(PAGE, sorted.length - visible)} more ({sorted.length - visible} remaining)
          </button>
        )}
      </div>

      {dlMsg && (
        <div className="dim" style={{ marginTop: 8, fontSize: 11 }}>{dlMsg}</div>
      )}
    </>
  );
}
