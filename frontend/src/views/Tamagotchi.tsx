import { useEffect, useState } from "react";
import * as api from "../api";

// Little companion that lives in the HUD and reports live host telemetry.
// Drop-in scope ("just for testing"): poll VM metrics and mirror them as mood.

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

  if (!m) return null;

  const vram = pct(m.vram_used_mb, m.vram_total_mb);
  const mem = pct(m.ram_used_mb, m.ram_total_mb);
  const load = Math.max(m.gpu_util, vram, mem, m.cpu_pct);
  const mood = load >= 88 ? "hot" : load >= 65 ? "busy" : load >= 35 ? "focus" : "calm";
  const tint =
    mood === "hot" ? "var(--oom)"
    : mood === "busy" ? "var(--warn)"
    : mood === "focus" ? "var(--cyan)"
    : "var(--pass)";

  const meters = [
    { label: "VRAM", v: vram, r: m.vram_used_mb, t: m.vram_total_mb },
    { label: "GPU", v: m.gpu_util, r: m.gpu_util, t: 100 },
    { label: "RAM", v: mem, r: m.ram_used_mb, t: m.ram_total_mb },
    { label: "CPU", v: m.cpu_pct, r: m.cpu_pct, t: 100 },
  ];

  return (
    <div
      style={{
        display: "flex",
        gap: 12,
        alignItems: "center",
        justifyContent: "center",
        background: "var(--panel)",
        border: "1px solid var(--line)",
        padding: "6px 12px",
        marginBottom: 8,
        userSelect: "none",
      }}
    >
      <style>{`
        .pet-bob { animation: petbob 3s ease-in-out infinite; transform-origin: 50% 100%; }
        @keyframes petbob { 0%,100% { transform: translateY(0) scaleY(1); }
                            50% { transform: translateY(-2px) scaleY(1.02); } }
        .pet-eyes { animation: petblink 4.5s infinite; transform-origin: 50% 50%; }
        .pet-late { animation-delay: 400ms; }
        @keyframes petblink { 0%,92%,100% { transform: scaleY(1); }
                              95%,97% { transform: scaleY(0.08); } }
        .pet-sweat { animation: petsweat 1s ease-in infinite; }
        @keyframes petsweat { 0% { transform: translateY(0); opacity: 0; }
                              30% { opacity: 1; }
                              100% { transform: translateY(7px); opacity: 0; } }
      `}</style>

      <button
        onClick={() => setOpen((o) => !o)}
        title={open ? "hide report" : mood + " — all good"}
        style={{ background: "none", border: "none", padding: 0, cursor: "pointer", lineHeight: 0 }}
      >
        <svg viewBox="0 0 72 60" width={open ? 56 : 40} height={open ? 47 : 33}>
          <g className="pet-bob">
            <ellipse cx="36" cy="40" rx="27" ry="17" fill={tint} opacity="0.85" />
            <path d="M18 48 q4 7 10 0" fill="none" stroke={tint} strokeWidth="2" />
            <path d="M54 48 q-4 7 -10 0" fill="none" stroke={tint} strokeWidth="2" />
            <g className="pet-eyes">
              <circle cx="26" cy="36" r="5.5" fill="#fff" />
              <circle cx="28.5" cy="37.5" r="2.6" fill="#111" />
              <circle cx="46" cy="36" r="5.5" fill="#fff" className="pet-late" />
              <circle cx="48.5" cy="37.5" r="2.6" fill="#111" className="pet-late" />
            </g>
            <path
              d={mood === "hot" ? "M33 47 q3 4 6 0" : "M33 47 q3 3 6 0"}
              fill="none"
              stroke="#111"
              strokeWidth="1.8"
              strokeLinecap="round"
            />
          </g>
          {mood === "hot" && (
            <g className="pet-sweat">
              <path d="M16 24 q4 6 0 8 q-4 -2 0 -8" fill="var(--cyan)" />
              <path d="M58 20 q4 6 0 8 q-4 -2 0 -8" fill="var(--cyan)" />
            </g>
          )}
        </svg>
      </button>

      {open && (
        <div className="row" style={{ gap: 10, flexWrap: "wrap" }}>
          {meters.map((s) => (
            <div key={s.label} title={`${s.r} / ${s.t}`} style={{ minWidth: 92 }}>
              <div className="dim" style={{ fontSize: 9, letterSpacing: 1 }}>
                {s.label} {s.v}%
              </div>
              <div style={{ width: "100%", height: 5, background: "var(--bg)", border: "1px solid var(--line)", borderRadius: 3, marginTop: 3 }}>
                <div
                  style={{
                    width: `${Math.max(2, s.v)}%`,
                    height: "100%",
                    background: tint,
                    borderRadius: 2,
                  }}
                />
              </div>
              <div className="mono dim" style={{ fontSize: 9, marginTop: 2 }}>
                {s.t >= 1000 ? `${(s.r / 1024).toFixed(1)}G` : `${s.r}%`}
                <span style={{ opacity: 0.5 }}> / {s.t >= 1000 ? `${(s.t / 1024).toFixed(1)}G` : "100%"}</span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}