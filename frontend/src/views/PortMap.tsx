import { useCallback, useEffect, useState } from "react";
import * as api from "../api";
import { latestBySlot, sortSlots, slotKey } from "../lib/portmap";

const STATE_COLOR: Record<string, string> = {
  up: "var(--pass)",
  starting: "var(--warn)",
  down: "var(--dim2)",
};

const VERDICT_COLOR: Record<string, string> = {
  PASS: "var(--pass)",
  WARN: "var(--warn)",
  OOM: "var(--oom)",
};

/** PORT MAP — the residency card. One row per engine's fixed slot: live
 * state, the profile bound to it (from the residents table), and the latest
 * recorded tok/s so you can see where to type before you type. Stopping a
 * slot clears its binding and leaves the other residents untouched.
 *
 * Also shows llama-server processes not in cyberdeck's slot system — these
 * may be truly ad-hoc or live in a user-managed systemd unit. The STOP
 * button uses systemctl stop when a unit is detected (avoiding restart
 * loops), or SIGTERM for raw processes. */
export default function PortMap({ onChanged }: { onChanged?: () => void }) {
  const [slots, setSlots] = useState<api.PortMapSlot[] | null>(null);
  const [bench, setBench] = useState<Map<string, import("../lib/portmap").SlotBench>>(
    () => new Map()
  );
  const [unmanaged, setUnmanaged] = useState<api.UnmanagedProcess[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [msg, setMsg] = useState("");

  const load = useCallback(async () => {
    const [s, hist, um] = await Promise.all([
      api.portMapStatus("127.0.0.1"),
      api.benchHistory(),
      api.unmanagedEngines(),
    ]);
    setSlots(sortSlots(s));
    setBench(latestBySlot(hist));
    setUnmanaged(um);
  }, []);

  useEffect(() => {
    load().catch((e) => setMsg(String(e)));
    const t = window.setInterval(() => void load().catch(() => {}), 15000);
    return () => window.clearInterval(t);
  }, [load]);

  const stop = async (engine: string) => {
    setBusy(engine);
    setMsg("");
    try {
      await api.engineStop(engine);
      await load();
      onChanged?.();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const start = async (engine: string) => {
    setBusy(engine);
    setMsg("");
    try {
      await api.engineStart(engine);
      await load();
      onChanged?.();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const stopPid = async (pid: number) => {
    setBusy(`pid:${pid}`);
    setMsg("");
    try {
      await api.unmanagedStop(pid);
      await load();
      onChanged?.();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  const startPid = async (pid: number) => {
    setBusy(`start:${pid}`);
    setMsg("");
    try {
      await api.unmanagedStart(pid);
      await load();
      onChanged?.();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="card" style={{ marginBottom: 10, fontSize: 11 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <h3 style={{ fontSize: 11, letterSpacing: 0.6, color: "var(--muted)", margin: 0 }}>LOADED MODELS</h3>
        <span className="dim" style={{ fontSize: 9 }}>
          LM Studio-style · start/stop each slot independently
        </span>
      </div>
      {!slots ? (
        <div className="dim" style={{ padding: 8 }}>probing slots…</div>
      ) : (
        slots.map((s) => {
          const b = bench.get(slotKey(s.engine, s.port));
          return (
            <div key={s.engine} className="row" style={{ gap: 8, marginTop: 8, alignItems: "center" }}>
              <span
                title={s.state}
                style={{
                  width: 7, height: 7, borderRadius: "50%", flexShrink: 0,
                  background: STATE_COLOR[s.state] ?? "var(--dim2)",
                  boxShadow: "none",
                }}
              />
              <span className="mono" style={{ width: 92, fontSize: 11 }}>{s.display}</span>
              <span className="mono dim" style={{ width: 58, fontSize: 10 }}>:{s.port}</span>
              <span className="mono" style={{ flex: 1, fontSize: 10, color: s.profile ? "var(--text)" : "var(--dim2)" }}>
                {s.profile ?? "unbound"}
                {s.resident && <span style={{ color: "var(--magenta)" }}> ·resident</span>}
              </span>
              <span className="mono" style={{ fontSize: 10, color: b ? "var(--cyan)" : "var(--dim2)", width: 92, textAlign: "right" }} title={b ? `${b.model} @ ctx ${b.ctx.toLocaleString()}` : "no bench reading"}>
                {b ? `${b.tps.toFixed(1)} tok/s` : "—"}
              </span>
              {s.fit_verdict && (
                <span
                  className="mono"
                  style={{
                    fontSize: 10,
                    color: VERDICT_COLOR[s.fit_verdict] ?? "var(--dim2)",
                    width: 54,
                    textAlign: "right",
                  }}
                  title={`fit: ${s.fit_verdict}`}
                >
                  {s.fit_verdict}
                </span>
              )}
              {s.state !== "down" ? (
                <button
                  className="ghost"
                  style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--oom)", color: "var(--oom)" }}
                  onClick={() => stop(s.engine)}
                  disabled={busy === s.engine}
                  title={`stop ${s.display} — clears slot, other residents stay up (LM Studio-style)`}
                >
                  {busy === s.engine ? "…" : "STOP"}
                </button>
              ) : s.profile ? (
                <button
                  className="ghost"
                  style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--pass)", color: "var(--pass)" }}
                  onClick={() => start(s.engine)}
                  disabled={busy === s.engine}
                  title={`start ${s.profile} on ${s.display} :${s.port}`}
                >
                  {busy === s.engine ? "…" : "START"}
                </button>
              ) : (
                <span className="dim" style={{ fontSize: 9, width: 38, textAlign: "center" }}>—</span>
              )}
            </div>
          );
        })
      )}

      {/* Ad-hoc processes not managed by systemd — these eat VRAM silently */}
      {unmanaged.length > 0 && (
        <>
          <div className="row" style={{ justifyContent: "space-between", alignItems: "center", marginTop: 10, borderTop: "1px solid var(--border)", paddingTop: 8 }}>
            <h3 style={{ fontSize: 10, letterSpacing: 0.6, color: "var(--warn)", margin: 0 }}>UNMANAGED PROCESSES</h3>
            <span className="dim" style={{ fontSize: 9 }}>
              not part of cyberdeck's slot system · free VRAM by stopping
            </span>
          </div>
          {unmanaged.map((p) => (
            <div key={p.pid} className="row" style={{ gap: 8, marginTop: 6, alignItems: "center" }}>
              <span
                title={p.systemd_unit ? `systemd unit: ${p.systemd_unit}` : "ad-hoc (no systemd)"}
                style={{
                  width: 7, height: 7, borderRadius: "50%", flexShrink: 0,
                  background: p.systemd_unit ? "var(--cyan)" : "var(--warn)",
                  boxShadow: "none",
                }}
              />
              <span className="mono" style={{ width: 92, fontSize: 11 }}>{p.display}</span>
              <span className="mono dim" style={{ width: 58, fontSize: 10 }}>
                {p.port != null ? `:${p.port}` : ""}
              </span>
              <span className="mono" style={{ flex: 1, fontSize: 10, color: p.systemd_unit ? "var(--cyan)" : "var(--dim2)" }}>
                {p.systemd_unit ?? `pid:${p.pid}`}
              </span>
              <span className="mono dim" style={{ fontSize: 9, width: 92, textAlign: "right" }}>
                {p.systemd_unit ? "systemd" : "ad-hoc"}
              </span>
              {p.systemd_unit && (
                <button
                  className="ghost"
                  style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--pass)", color: "var(--pass)" }}
                  onClick={() => startPid(p.pid)}
                  disabled={busy === `start:${p.pid}`}
                  title={`systemctl start ${p.systemd_unit}`}
                >
                  {busy === `start:${p.pid}` ? "…" : "START"}
                </button>
              )}
              <button
                className="ghost"
                style={{ fontSize: 9, padding: "3px 7px", borderColor: "var(--oom)", color: "var(--oom)" }}
                onClick={() => stopPid(p.pid)}
                disabled={busy === `pid:${p.pid}`}
                title={p.systemd_unit ? `systemctl stop ${p.systemd_unit}` : `kill pid ${p.pid} — sends SIGTERM`}
              >
                {busy === `pid:${p.pid}` ? "…" : "STOP"}
              </button>
            </div>
          ))}
        </>
      )}

      {msg && <div style={{ color: "var(--oom)", marginTop: 8, fontSize: 10 }}>{msg}</div>}
    </div>
  );
}
