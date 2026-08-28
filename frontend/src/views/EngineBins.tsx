import { useEffect, useState } from "react";
import * as api from "../api";

/** Per-engine executable config. Set on the machine that serves models — a
 * configured bin is used by bringup / test / matrix whenever a profile's
 * default binary doesn't exist on disk (e.g. a stock /usr/bin/llama-server). */
export default function EngineBins({ onDone }: { onDone?: () => void }) {
  const [rows, setRows] = useState<api.EngineBinRow[] | null>(null);
  const [vals, setVals] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState<string | null>(null);
  const [msg, setMsg] = useState("");

  const load = async () => {
    const r = await api.engineBinList();
    setRows(r);
    setVals(Object.fromEntries(r.map((x) => [x.engine_id, x.bin ?? ""])));
  };

  useEffect(() => {
    load().catch((e) => setMsg(String(e)));
  }, []);

  if (!rows) return <div className="dim" style={{ fontSize: 11, padding: 8 }}>engine binaries…</div>;

  const save = async (id: string) => {
    setSaving(id);
    setMsg("");
    try {
      const v = vals[id].trim();
      if (v === "") await api.engineBinClear(id);
      else await api.engineBinSet(id, v);
      await load();
      onDone?.();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setSaving(null);
    }
  };

  return (
    <div className="card" style={{ marginBottom: 10, fontSize: 11 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <h3 style={{ fontSize: 11, letterSpacing: 2, color: "var(--cyan)", margin: 0 }}>ENGINE BINARIES</h3>
        <span className="dim" style={{ fontSize: 9 }}>used by test/bringup/matrix when a profile's bin is missing</span>
      </div>
      {rows.map((r) => (
        <div key={r.engine_id} className="row" style={{ gap: 8, marginTop: 8 }}>
          <span className="mono" style={{ width: 84, fontSize: 11 }}>{r.engine_id}</span>
          <input
            value={vals[r.engine_id] ?? ""}
            onChange={(e) => setVals((v) => ({ ...v, [r.engine_id]: e.target.value }))}
            placeholder={`(engine default: ${r.engine_id === "llamacpp" ? "llama-server" : r.engine_id === "freetoken" ? "ft" : "ollama"})`}
            style={{ flex: 1, background: "#0e0e18", border: "1px solid #232336", color: "var(--text)", padding: "4px 8px", fontSize: 11, fontFamily: "monospace" }}
          />
          <button
            className="ghost"
            style={{ fontSize: 10, padding: "4px 8px" }}
            onClick={() => save(r.engine_id)}
            disabled={saving === r.engine_id}
          >
            {saving === r.engine_id ? "…" : "SAVE"}
          </button>
        </div>
      ))}
      {msg && (
        <div style={{ color: "var(--oom)", marginTop: 8, fontSize: 10 }}>{msg}</div>
      )}
      <div className="dim" style={{ fontSize: 9, marginTop: 8 }}>
        leave empty to use the engine's default resolution · an empty save clears the config
      </div>
    </div>
  );
}