import { useEffect, useState } from "react";
import * as api from "../api";

/** Runtime installs + updates: fetch a backend from its manifest recipe, or
 * print manual guidance. Sits under the binary overrides — same management
 * surface, one concern per card. */
function RuntimeInstalls({ onDone }: { onDone?: () => void }) {
  const [rts, setRts] = useState<api.RuntimeRow[] | null>(null);
  const [updates, setUpdates] = useState<api.RuntimeUpdate[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [msg, setMsg] = useState("");

  const load = async () => setRts(await api.runtimeList());

  useEffect(() => {
    load().catch((e) => setMsg(String(e)));
  }, []);

  const install = async (id: string, display: string) => {
    if (!confirm(`Install runtime '${display}'?\n\nDownloads the release artifact (can be hundreds of MB), unpacks it under ~/.local/share/cyberdeck/backends, and registers the binary.`)) return;
    setBusy(id);
    setMsg("");
    try {
      const rep = await api.runtimeInstall({ id });
      alert(rep.summary);
      await load();
      onDone?.();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const checkUpdates = async () => {
    setBusy("updates");
    setMsg("");
    try {
      setUpdates(await api.runtimeCheckUpdates());
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const noteFor = (id: string) => updates?.find((u) => u.id === id);

  return (
    <div className="card" style={{ marginBottom: 10, fontSize: 11 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--muted)", margin: 0 }}>RUNTIME INSTALLS</h3>
        <button className="ghost" style={{ fontSize: 9, padding: "3px 8px" }} onClick={() => checkUpdates()} disabled={busy !== null}>
          {busy === "updates" ? "…" : "check updates"}
        </button>
      </div>
      {!rts && <div className="dim" style={{ fontSize: 10, marginTop: 6 }}>runtimes…</div>}
      {rts?.map((r) => {
        const note = noteFor(r.id);
        return (
          <div key={r.id} className="row" style={{ gap: 8, marginTop: 8, alignItems: "center" }}>
            <span className="mono" style={{ width: 84, fontSize: 11 }}>{r.display}</span>
            <span
              className="mono dim"
              style={{ flex: 1, fontSize: 10, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
              title={r.bin ?? "no binary resolves"}
            >
              {r.installed ? r.bin : "missing — no binary resolves"}
            </span>
            {note && (
              <span style={{ fontSize: 9, color: note.update_available ? "var(--warn)" : "var(--dim2)" }} title={note.note}>
                {note.update_available ? `update: ${note.installed_version} → ${note.latest}` : note.note}
              </span>
            )}
            <button
              className="ghost"
              style={{ fontSize: 10, padding: "4px 8px" }}
              onClick={() => install(r.id, r.display)}
              disabled={busy !== null}
              title={r.installed ? `re-install ${r.display} (fetches newest)` : `install ${r.display}`}
            >
              {busy === r.id ? "…" : r.installed ? "REINSTALL" : "INSTALL"}
            </button>
          </div>
        );
      })}
      {msg && <div style={{ color: "var(--oom)", marginTop: 8, fontSize: 10 }}>{msg}</div>}
      <div className="dim" style={{ fontSize: 9, marginTop: 8 }}>
        recipes live in the runtime manifests · manual backends print guidance instead of downloading
      </div>
    </div>
  );
}

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
    <>
      <div className="card" style={{ marginBottom: 10, fontSize: 11 }}>
        <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
          <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--muted)", margin: 0 }}>ENGINE BINARIES</h3>
          <span className="dim" style={{ fontSize: 9 }}>used by test/bringup/matrix when a profile's bin is missing</span>
        </div>
        {rows.map((r) => (
          <div key={r.engine_id} className="row" style={{ gap: 8, marginTop: 8 }}>
            <span className="mono" style={{ width: 84, fontSize: 11 }}>{r.engine_id}</span>
            <input
              value={vals[r.engine_id] ?? ""}
              onChange={(e) => setVals((v) => ({ ...v, [r.engine_id]: e.target.value }))}
              placeholder={`(engine default: ${r.engine_id === "llamacpp" ? "llama-server" : r.engine_id === "freetoken" ? "ft" : "ollama"})`}
              style={{ flex: 1, background: "var(--bg)", border: "1px solid var(--line)", color: "var(--text)", padding: "4px 8px", fontSize: 11, fontFamily: "monospace" }}
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
      <RuntimeInstalls onDone={onDone} />
    </>
  );
}
