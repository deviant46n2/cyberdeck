import { useEffect, useState } from "react";
import * as api from "../api";

// Companion that lives pinned to the sidebar bottom, always visible.
// Polls live host telemetry and mirrors it as the pet's mood.

const POLL_MS = 2000;

function pct(a: number, b: number): number {
  return b > 0 ? Math.round((a / b) * 100) : 0;
}

export default function Tamagotchi() {
  const [m, setM] = useState<api.LiveMetrics | null>(null);
  const [open, setOpen] = useState(true);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const r = await api.hostMetrics();
        if (alive) setM(r);
      } catch {
        /* engine untouched — next tick retries */
      }
    };
    void tick();
    const t = setInterval(tick, POLL_MS);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, []);

  const vram = m ? pct(m.vram_used_mb, m.vram_total_mb) : 0;
  const mem = m ? pct(m.ram_used_mb, m.ram_total_mb) : 0;
  const load = m ? Math.max(m.gpu_util, vram, mem, m.cpu_pct) : 0;
  const mood = load >= 88 ? "hot" : load >= 65 ? "busy" : load >= 35 ? "focus" : "calm";
  const tint =
    mood === "hot" ? "var(--oom)"
    : mood === "busy" ? "var(--warn)"
    : mood === "focus" ? "var(--cyan)"
    : "var(--pass)";

  const meters = m
    ? [
        { label: "VRAM", v: vram, r: m.vram_used_mb, t: m.vram_total_mb },
        { label: "GPU", v: m.gpu_util, r: m.gpu_util, t: 100 },
        { label: "RAM", v: mem, r: m.ram_used_mb, t: m.ram_total_mb },
        { label: "CPU", v: m.cpu_pct, r: m.cpu_pct, t: 100 },
      ]
    : [];

  const title = m
    ? `VRAM ${vram}% · GPU ${m.gpu_util}% · RAM ${mem}% · CPU ${m.cpu_pct}%`
    : "telemetry loading…";

  return (
    <div
      style={{
        borderTop: "1px solid var(--line)",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 3,
        padding: "8px 2px 3px",
        margin: "0 2px",
      }}
    >
      <style>{`
        .cp-bob { animation: cpbob 3s ease-in-out infinite; transform-origin: 50% 100%; }
        @keyframes cpbob { 0%,100% { transform: translateY(0); } 50% { transform: translateY(-1.5px); } }
        .cp-eyes { animation: cpblink 4.5s infinite; transform-origin: 50% 50%; }
        .cp-late { animation-delay: 400ms; }
        @keyframes cpblink { 0%,92%,100% { transform: scaleY(1); } 95%,97% { transform: scaleY(0.08); } }
        .cp-sweat { animation: cpsweat 1s ease-in infinite; }
        @keyframes cpsweat { 0% { transform: translateY(0); opacity: 0; } 30% { opacity: 1; }
                             100% { transform: translateY(6px); opacity: 0; } }
      `}</style>

      <button
        onClick={() => setOpen((o) => !o)}
        title={open ? title : mood + " — " + title}
        style={{ background: "none", border: "none", padding: 0, cursor: "pointer", lineHeight: 0 }}
      >
        <svg viewBox="0 0 72 60" width={open ? 40 : 32} height={open ? 33 : 27}>
          <g className="cp-bob">
            <ellipse cx="36" cy="40" rx="27" ry="17" fill={tint} opacity="0.85" />
            <path d="M18 48 q4 7 10 0" fill="none" stroke={tint} strokeWidth="3" />
            <path d="M54 48 q-4 7 -10 0" fill="none" stroke={tint} strokeWidth="3" />
            <g className="cp-eyes">
              <circle cx="26" cy="36" r="5.5" fill="#fff" />
              <circle cx="28.5" cy="37.5" r="2.6" fill="#111" />
              <circle cx="46" cy="36" r="5.5" fill="#fff" className="cp-late" />
              <circle cx="48.5" cy="37.5" r="2.6" fill="#111" className="cp-late" />
            </g>
            <path
              d={mood === "hot" ? "M33 47 q3 4 6 0" : "M33 47 q3 3 6 0"}
              fill="none"
              stroke="#111"
              strokeWidth="2"
              strokeLinecap="round"
            />
          </g>
          {mood === "hot" && (
            <g className="cp-sweat">
              <path d="M16 24 q4 6 0 8 q-4 -2 0 -8" fill="var(--cyan)" />
              <path d="M58 20 q4 6 0 8 q-4 -2 0 -8" fill="var(--cyan)" />
            </g>
          )}
        </svg>
      </button>

      {open && m && (
        <div style={{ display: "flex", flexDirection: "column", gap: 5, width: "100%", padding: "4px 8px" }}>
          {meters.map((s) => (
            <div key={s.label} title={`${s.r} / ${s.t}`}>
              <div style={{ display: "flex", justifyContent: "space-between", fontSize: 9, letterSpacing: 0.5 }}>
                <span className="dim">{s.label}</span>
                <span>{s.v}%</span>
              </div>
              <div style={{ width: "100%", height: 4, background: "var(--bg)", border: "1px solid var(--line)", borderRadius: 3, marginTop: 2 }}>
                <div
                  style={{ width: `${Math.max(2, s.v)}%`, height: "100%", background: tint, borderRadius: 2 }}
                />
              </div>
            </div>
          ))}
          <div className="dim mono" style={{ fontSize: 8.5, textAlign: "center", paddingTop: 2 }}>
            {m.vram_used_mb >= 1024 ? `${(m.vram_used_mb / 1024).toFixed(1)}G` : `${m.vram_used_mb}M`} VRAM used
          </div>
        </div>
      )}
    </div>
  );
}