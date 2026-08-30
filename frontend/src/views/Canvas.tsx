import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import * as api from "../api";

const KIND_COLOR: Record<string, string> = {
  Agentic: "var(--magenta)",
  Stateless: "var(--cyan)",
};

/** CANVAS — Infinite Agent Canvas (ROADMAP 8d).
 *
 * A minimal DOM renderer for saved workflows (the node-based DAG from the
 * headless `deck workflow` foundation): node cards positioned by their stored
 * `pos`, SVG edges for the graph, and a RUN/STOP door that fans out to the
 * background Tauri executor behind `wf-*` events. Self-fetching like the
 * PORT MAP card — no global state.
 */
export default function Canvas() {
  const [workflows, setWorkflows] = useState<api.Workflow[]>([]);
  const [selected, setSelected] = useState<string>("");
  const [busy, setBusy] = useState<string | null>(null);
  const [runner, setRunner] = useState<"stateless" | "agentic">("stateless");
  const [dir, setDir] = useState("");
  const [msg, setMsg] = useState("");
  const [status, setStatus] = useState<Map<string, string>>(new Map());
  const [history, setHistory] = useState<api.WfRunRow[]>([]);
  const [bench, setBench] = useState<api.RoleBenchRow[]>([]);

  const load = useCallback(async () => {
    try {
      await api.workflowSeed();
      const wfs = await api.workflowList();
      setWorkflows(wfs);
      if (!selected && wfs.length > 0) setSelected(wfs[0].id);
      const h = await api.workflowHistory();
      setHistory(h);
    } catch (e) {
      setMsg(String(e));
    }
  }, [selected]);

  useEffect(() => {
    let un: UnlistenFn[] = [];
    let mounted = true;
    (async () => {
      const listenNode = await listen<api.WfNodeEvt>("wf-node", (e) => {
        if (!mounted) return;
        setStatus((m) => {
          const next = new Map(m);
          if (e.payload.skipped) next.set(e.payload.node_id, "skipped");
          else next.set(e.payload.node_id, e.payload.ok ? "ok" : `ERR ${e.payload.error}`);
          return next;
        });
      });
      const listenDone = await listen<api.WfDoneEvt>("wf-done", (e) => {
        if (!mounted) return;
        setBusy(null);
        const iters = e.payload.iterations ? ` · ${e.payload.iterations} loop iterations` : "";
        setMsg(`${e.payload.status}: ${e.payload.nodes_ok} ok / ${e.payload.nodes_failed} failed · ${e.payload.tokens_used} tokens${iters}`);
        load();
      });
      un = [listenNode, listenDone];
    })();
    load().catch(() => {});
    return () => {
      mounted = false;
      un.forEach((u) => u());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!selected) {
      setBench([]);
      return;
    }
    api.workflowPerRoleBench(selected).then(setBench).catch(() => setBench([]));
  }, [selected]);

  const run = async () => {
    if (!selected) return;
    setBusy("run");
    setStatus(new Map());
    setMsg("");
    try {
      await api.workflowRun(selected, runner, dir ? dir : null);
      setMsg(`workflow '${selected}' queued…`);
    } catch (e) {
      setMsg(String(e));
      setBusy(null);
    }
  };

  const stop = async () => {
    const runRows = history.filter((r) => r.workflow_id === selected && r.status === "Running");
    if (runRows.length === 0) {
      setMsg("no running run found for this workflow");
      return;
    }
    try {
      await api.workflowStop(runRows[runRows.length - 1].id);
      setMsg("requested stop…");
    } catch (e) {
      setMsg(String(e));
    }
  };

  const wf = workflows.find((w) => w.id === selected);
  const bounds = wf ? layoutBounds(wf.nodes) : { minX: 0, minY: 0, maxX: 800, maxY: 500 };

  return (
    <div>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <h2 className="viewtypo">CANVAS</h2>
        <span className="dim" style={{ fontSize: 10 }}>
          node-based multi-model workflows · roles bound to models, fanned by the DAG
        </span>
      </div>

      <div className="row" style={{ gap: 10, marginTop: 12, alignItems: "flex-start" }}>
        {/* workflow list */}
        <div className="card" style={{ width: 240, fontSize: 11 }}>
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
            <h3 style={{ fontSize: 11, letterSpacing: 2, color: "var(--cyan)", margin: 0 }}>WORKFLOWS</h3>
            <button className="ghost" style={{ fontSize: 9, padding: "2px 6px" }} onClick={() => { setSelected(""); load(); }} title="reseed + reload">
              RESEED
            </button>
          </div>
          {workflows.length === 0 && <div className="dim" style={{ padding: 6 }}>seeding…</div>}
          {workflows.map((w) => (
            <div
              key={w.id}
              className="row"
              onClick={() => { setSelected(w.id); setMsg(""); }}
              style={{ gap: 6, padding: "5px 6px", cursor: "pointer", borderRadius: 4, background: selected === w.id ? "var(--panel-2)" : "transparent" }}
            >
              <span style={{ color: "var(--cyan)", fontWeight: 600 }}>{w.name}</span>
              <span className="dim" style={{ fontSize: 9 }}>{w.nodes.length}n {w.edges.length}e</span>
            </div>
          ))}
        </div>

        {/* canvas */}
        <div className="card" style={{ flex: 1, fontSize: 11 }}>
          {!wf ? (
            <div className="dim" style={{ padding: 12 }}>no workflow selected — seed one first</div>
          ) : (
            <>
              <div className="row" style={{ justifyContent: "space-between", alignItems: "center", flexWrap: "wrap", gap: 8 }}>
                <span style={{ fontSize: 13 }}>{wf.name} <span className="dim">· {wf.description}</span></span>
                <div className="row" style={{ gap: 6, alignItems: "center" }}>
                  <select
                    value={runner}
                    onChange={(e) => setRunner(e.target.value as "stateless" | "agentic")}
                    style={{ fontSize: 10, background: "var(--panel-2)", color: "var(--text)", border: "1px solid var(--dim2)", borderRadius: 4, padding: "3px 6px" }}
                  >
                    <option value="stateless">stateless</option>
                    <option value="agentic">agentic</option>
                  </select>
                  {runner === "agentic" && (
                    <input
                      value={dir}
                      placeholder="/path/to/workspace"
                      onChange={(e) => setDir(e.target.value)}
                      style={{ fontSize: 10, width: 160, background: "var(--panel-2)", color: "var(--text)", border: "1px solid var(--dim2)", borderRadius: 4, padding: "3px 6px" }}
                    />
                  )}
                  <button className="ghost" style={{ color: "var(--pass)", borderColor: "var(--pass)", fontSize: 10 }} onClick={run} disabled={busy === "run"}>
                    {busy === "run" ? "RUNNING…" : "▶ RUN"}
                  </button>
                  <button className="ghost" style={{ color: "var(--oom)", borderColor: "var(--oom)", fontSize: 10 }} onClick={stop}>
                    ■ STOP
                  </button>
                </div>
              </div>

              <div
                style={{
                  position: "relative",
                  marginTop: 12,
                  height: Math.max(bounds.maxY + 120, 220),
                  background: "var(--panel-2)",
                  borderRadius: 6,
                  overflow: "hidden",
                }}
              >
                {/* edges */}
                <svg style={{ position: "absolute", left: 0, top: 0, width: "100%", height: "100%", pointerEvents: "none" }}>
                  {wf.edges.map((e) => {
                    const a = wf.nodes.find((n) => n.id === e.from);
                    const b = wf.nodes.find((n) => n.id === e.to);
                    if (!a || !b) return null;
                    const x1 = NODE_W / 2 + a.pos.x;
                    const y1 = NODE_H / 2 + a.pos.y;
                    const x2 = NODE_W / 2 + b.pos.x;
                    const y2 = NODE_H / 2 + b.pos.y;
                    const ct = (a.pos.y < b.pos.y ? 1 : -1) * 40;
                    const edgeLabel = e.loop_edge ? "⟲ loop" : e.condition ? `? ${e.condition}` : null;
                    const col = e.loop_edge ? "var(--magenta)" : e.condition ? "var(--warn, #d9a441)" : "var(--dim2)";
                    return (
                      <g key={e.id}>
                        <path
                          d={`M ${x1} ${y1} C ${x1} ${y1 + ct}, ${x2} ${y2 - ct}, ${x2} ${y2}`}
                          fill="none"
                          stroke={col}
                          strokeWidth={1.5}
                        />
                        {edgeLabel && (
                          <text
                            x={(x1 + x2) / 2}
                            y={(y1 + y2) / 2 - 6}
                            textAnchor="middle"
                            fontSize={9}
                            fill={col}
                            fontFamily="var(--font-mono, monospace)"
                          >
                            {edgeLabel}
                          </text>
                        )}
                      </g>
                    );
                  })}
                </svg>

                {wf.nodes.map((n) => {
                  const st = status.get(n.id);
                  return (
                    <div
                      key={n.id}
                      style={{
                        position: "absolute",
                        left: n.pos.x,
                        top: n.pos.y,
                        width: NODE_W,
                        height: NODE_H,
                        padding: "8px 10px",
                        background: "var(--panel)",
                        border: `1px solid ${st === "ok" ? "var(--pass)" : st === "skipped" ? "var(--warn, #d9a441)" : st ? "var(--oom)" : "var(--dim2)"}`,
                        borderRadius: 6,
                        boxShadow: "0 2px 10px rgba(0,0,0,0.35)",
                        display: "flex",
                        flexDirection: "column",
                        gap: 4,
                      }}
                    >
                      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
                        <span className="mono" style={{ color: "var(--cyan)", fontSize: 11 }}>{n.role_id}</span>
                        <span className="mono" style={{ fontSize: 8, color: KIND_COLOR[n.kind] ?? "var(--dim2)" }}>{n.kind}</span>
                      </div>
                      <span className="mono" style={{ fontSize: 9, color: "var(--text)" }}>{n.binding.model_ref}</span>
                      {n.binding.engine && <span className="mono dim" style={{ fontSize: 8 }}>@ {n.binding.engine}</span>}
                      {st && <span className="mono" style={{ fontSize: 8, color: st === "ok" ? "var(--pass)" : st === "skipped" ? "var(--warn, #d9a441)" : "var(--oom)" }}>{st}</span>}
                    </div>
                  );
                })}
              </div>
            </>
          )}
          {msg && <div style={{ color: "var(--oom)", marginTop: 8, fontSize: 10 }}>{msg}</div>}
        </div>
      </div>

      {/* per-role bench (8e) — which model best at which node */}
      <div className="card" style={{ marginTop: 10, fontSize: 11 }}>
        <h3 style={{ fontSize: 11, letterSpacing: 2, color: "var(--cyan)", margin: 0, marginBottom: 6 }}>
          PER-ROLE BENCH <span className="dim">· best tok/s across runs, per node</span>
        </h3>
        {bench.length === 0 ? (
          <div className="dim">no per-role bench yet — run this workflow (stateless) a few times to accumulate tok/s per node</div>
        ) : (
          <table>
            <thead>
              <tr>
                <th>ROLE</th>
                <th>MODEL</th>
                <th>BEST</th>
                <th>AVG</th>
                <th>LAST</th>
                <th>RUNS</th>
              </tr>
            </thead>
            <tbody>
              {bench.map((b) => {
                const bestInRole = bench
                  .filter((x) => x.role_id === b.role_id)
                  .every((x) => b.best_tps >= x.best_tps);
                return (
                  <tr key={`${b.role_id}:${b.model}:${b.engine}`}>
                    <td className="mono" style={{ color: "var(--cyan)" }}>{b.role_id}</td>
                    <td className="mono">
                      {b.model}
                      {bestInRole && <span className="badge pass" style={{ marginLeft: 6 }}>BEST</span>}
                    </td>
                    <td className="mono" style={{ color: "var(--pass)" }}>{b.best_tps.toFixed(1)}</td>
                    <td className="mono dim">{b.avg_tps.toFixed(1)}</td>
                    <td className="mono dim">{b.last_tps.toFixed(1)}</td>
                    <td className="mono dim">{b.runs}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

      {/* history */}
      <div className="card" style={{ marginTop: 10, fontSize: 11 }}>
        <h3 style={{ fontSize: 11, letterSpacing: 2, color: "var(--cyan)", margin: 0, marginBottom: 6 }}>RUN HISTORY</h3>
        {history.length === 0 && <div className="dim">no runs yet</div>}
        {history.map((r) => (
          <div key={r.id} className="row" style={{ gap: 8, padding: "3px 0" }}>
            <span className="mono dim" style={{ width: 170 }}>{r.id}</span>
            <span className="mono" style={{ width: 120, color: r.status === "Done" ? "var(--pass)" : r.status === "Partial" ? "var(--warn)" : "var(--text)" }}>{r.status}</span>
            <span className="mono dim" style={{ width: 80 }}>{r.workflow_id}</span>
            <span className="mono dim">{r.tokens_used} tok</span>
          </div>
        ))}
      </div>
    </div>
  );
}

const NODE_W = 190;
const NODE_H = 92;

function layoutBounds(nodes: api.WorkflowNode[]) {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const n of nodes) {
    minX = Math.min(minX, n.pos.x);
    minY = Math.min(minY, n.pos.y);
    maxX = Math.max(maxX, n.pos.x + NODE_W);
    maxY = Math.max(maxY, n.pos.y + NODE_H);
  }
  if (!isFinite(minX)) return { minX: 0, minY: 0, maxX: NODE_W, maxY: NODE_H };
  return { minX, minY, maxX, maxY };
}
