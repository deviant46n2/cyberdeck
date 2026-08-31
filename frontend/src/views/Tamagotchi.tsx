import { useEffect, useRef, useState } from "react";
import * as api from "../api";
import {
  advance,
  canFeed,
  epochSecs,
  faceFrom,
  feed,
  pet,
  petFromRaw,
  PET_LAST_FED_KEY,
  PET_LAST_SEEN_KEY,
  PET_LOVE_KEY,
  PET_HUNGER_KEY,
  type PetFace,
} from "../lib/pet";
import { animClass, pickIdleAnim, type PetAnim } from "../lib/pet-anim";

// Companion that lives pinned to the sidebar bottom, always visible.
// Polls live host telemetry, mirrors it as the pet's mood, and runs a living
// idle-animation loop. Feeds/pets update a persisted (settings-DB) love+hunger.

const POLL_MS = 2000;

function pct(a: number, b: number): number {
  return b > 0 ? Math.round((a / b) * 100) : 0;
}

// --- mood from host load -------------------------------------------
type LoadMood = "hot" | "busy" | "focus" | "calm";

function clip(v: number) {
  return Math.max(0, Math.min(100, v));
}

/** The resting animation used between idle behaviours. */
const IDLE: PetAnim = { id: "idle", weight: 0, ms: 3600, cycles: 1, label: "idle" };

function faceLabel(face: PetFace, love: number, hunger: number): string {
  switch (face) {
    case "starving":
      return "starving — feed me! 💀";
    case "hangry":
      return "getting hungry…";
    case "blissful":
      return "blissful ♥";
    case "loved":
      return "feeling loved";
    default:
      return hunger > 50 ? `a bit peckish (♥${clip(Math.round(love))})` : "happy blob";
  }
}

function moodShade(mood: LoadMood): string {
  switch (mood) {
    case "hot":
      return "#700";
    case "busy":
      return "#553";
    default:
      return "#111";
  }
}

/**
 * The full keyframe library. Layers:
 *   .cp-whole  — whole-body (translate/skew/rotate): walk, sway, flip, spin, hop, bounce, dance, tilt, stretch
 *   .cp-body   — body squash & stretch breathing / wobble
 *   .cp-legs   — foot shuffle / dangle
 *   .cp-eyes   — blink (single + double)
 *   .cp-pupils — eye darting
 *   .cp-mouth  — shape swap (smile / o / meh) per face
 *   .cp-cheeks — happy-cheek bloom
 *   .cp-sweat / .cp-hearts / .cp-starve — mood accessories
 */

const keyframes = `
  /* ---- resting: gentle breathing bob (always on when not playing) ---- */
  .cp-idle .cp-body { animation: kfBreath 3.4s ease-in-out infinite; transform-origin: 50% 100%; }
  @keyframes kfBreath { 0%,100%{transform:scaleY(1) scaleX(1);} 50%{transform:scaleY(1.02) scaleX(0.985) translateY(-0.5px);} }

  /* ---- the eye blink (applies regardless of whole-body anim) ---- */
  .cp-eyes { animation: kfBlink 4.5s infinite; transform-origin: 50% 50%; }
  .cp-late { animation-delay: 380ms; }
  @keyframes kfBlink { 0%,91%,100%{transform:scaleY(1);} 94%,96%{transform:scaleY(0.08);} }

  /* ---- IDLE ANIMATION CLASSES (whole-body drive) ---- */

  /* breath — deep, slow standing breathe */
  .cp-anim-breath .cp-body { animation: kfBreathDeep 3.6s ease-in-out infinite; transform-origin:50% 100%; }
  @keyframes kfBreathDeep { 0%,100%{transform:scaleY(1) scaleX(1) translateY(0);} 50%{transform:scaleY(1.05) scaleX(0.96) translateY(-1.2px);} }

  /* sway — rock side to side, feet planted */
  .cp-anim-sway .cp-whole { animation: kfSway 2.4s ease-in-out infinite; transform-origin:50% 100%; }
  @keyframes kfSway { 0%,100%{transform:rotate(0deg);} 25%{transform:rotate(-4deg) translateY(-0.5px);} 75%{transform:rotate(4deg) translateY(-0.5px);} }

  /* dart — eyes look around (edges snap, smooth ease) */
  .cp-anim-dart .cp-pupils { animation: kfDart 1.3s ease-in-out infinite; }
  @keyframes kfDart { 0%,12%{transform:translate(0,0);} 25%{transform:translate(-2.5px,-1.5px);} 37%,49%{transform:translate(2px,-1.5px);} 62%{transform:translate(0,0);} 74%{transform:translate(0,1.5px);} 85%,100%{transform:translate(-1.5px,1px);} }

  /* wobble — squashy jelly jiggle */
  .cp-anim-wobble .cp-body { animation: kfWobble 0.9s cubic-bezier(.36,.07,.19,.97) infinite; transform-origin:50% 100%; }
  @keyframes kfWobble { 0%,100%{transform:scaleX(1) scaleY(1);} 20%{transform:scaleX(1.08) scaleY(0.92);} 45%{transform:scaleX(0.94) scaleY(1.05);} 70%{transform:scaleX(1.04) scaleY(0.96);} }

  /* stretch — reach up and relax, feet dangle */
  .cp-anim-stretch .cp-whole { animation: kfStretch 2.2s ease-in-out infinite; transform-origin:50% 100%; }
  .cp-anim-stretch .cp-body { animation: kfStretchBody 2.2s ease-in-out infinite; transform-origin:50% 100%; }
  .cp-anim-stretch .cp-legs { animation: kfStretchLegs 2.2s ease-in-out infinite; }
  @keyframes kfStretch { 0%,45%,100%{transform:translateY(0);} 60%,80%{transform:translateY(-3px);} }
  @keyframes kfStretchBody { 0%,45%,100%{transform:scaleY(1);} 60%,80%{transform:scaleY(1.09) scaleX(0.96);} }
  @keyframes kfStretchLegs { 0%,45%,100%{transform:translateY(0);} 60%,80%{transform:translateY(3px);} }

  /* hop — a single playful jump with squash landing */
  .cp-anim-hop .cp-whole { animation: kfHop 1.4s cubic-bezier(.28,.84,.42,1) infinite; }
  @keyframes kfHop {
    0% { transform:translateY(0) scale(1,1); }
    35% { transform:translateY(-9px) scale(0.98,1.06); }
    55% { transform:translateY(0) scale(1.1,0.9); }
    75%,100% { transform:translateY(0) scale(1,1); }
  }

  /* bounce — happy springy series */
  .cp-anim-bounce .cp-whole { animation: kfBounce 2.3s ease-in-out infinite; }
  @keyframes kfBounce {
    0%,20%,50%,80%,100% { transform:translateY(0); }
    30% { transform:translateY(-6px) scale(0.99,1.05); }
    60% { transform:translateY(-4px) scale(0.99,1.04); }
    90% { transform:translateY(-2px); }
  }

  /* walk — translate across the bar with foot shuffles. Uses overflow-visible
     so the guy can roam past the viewBox edge, then come home. */
  .cp-stage { overflow: visible; }
  .cp-anim-walkL .cp-whole { animation: kfWalkL 3s linear infinite; }
  .cp-anim-walkR .cp-whole { animation: kfWalkR 3s linear infinite; }
  .cp-anim-walkL .cp-body, .cp-anim-walkR .cp-body { animation: kfWalkBob 0.5s ease-in-out infinite; }
  .cp-anim-walkL .cp-legs, .cp-anim-walkR .cp-legs { animation: kfWalkLegs 0.5s ease-in-out infinite; }
  @keyframes kfWalkL { 0%{transform:translateX(18px);} 50%{transform:translateX(-18px);} 100%{transform:translateX(18px);} }
  @keyframes kfWalkR { 0%{transform:translateX(-18px);} 50%{transform:translateX(18px);} 100%{transform:translateX(-18px);} }
  @keyframes kfWalkBob { 0%,100%{transform:translateY(0);} 50%{transform:translateY(-1.5px);} }
  @keyframes kfWalkLegs { 0%,100%{transform:translate(0,0);} 50%{transform:translate(2px,1px);} }

  /* flip — a fast full somersault */
  .cp-anim-flip .cp-whole { animation: kfFlip 1.1s cubic-bezier(.45,.05,.55,.95) infinite; transform-origin:50% 60%; }
  @keyframes kfFlip { 0%{transform:rotate(-10deg);} 25%{transform:rotate(80deg) translateY(-2px);} 50%{transform:rotate(180deg);} 75%{transform:rotate(280deg) translateY(-2px);} 100%{transform:rotate(360deg);} }

  /* spin — playful 360 with wobble */
  .cp-anim-spin .cp-whole { animation: kfSpin 1.5s ease-in-out infinite; transform-origin:50% 60%; }
  @keyframes kfSpin { 0%{transform:rotate(0deg) scale(1,1);} 50%{transform:rotate(180deg) scale(1.05,0.95);} 100%{transform:rotate(360deg) scale(1,1);} }

  /* tilt — curious lean left then right */
  .cp-anim-tilt .cp-whole { animation: kfTilt 2s ease-in-out infinite; transform-origin:50% 100%; }
  @keyframes kfTilt { 0%,100%{transform:rotate(0deg);} 25%{transform:rotate(9deg) translateY(0.5px);} 50%{transform:rotate(0deg);} 75%{transform:rotate(-9deg) translateY(0.5px);} }

  /* sleepy — heavy-lidded rock, tiny droop */
  .cp-anim-sleepy .cp-whole { animation: kfSleepy 4s ease-in-out infinite; transform-origin:50% 100%; }
  .cp-anim-sleepy .cp-eyes { animation: kfSleepBlink 1.6s ease-in-out infinite; }
  @keyframes kfSleepy { 0%,100%{transform:rotate(0deg) translateY(0);} 50%{transform:rotate(-3deg) translateY(0.6px);} }
  @keyframes kfSleepBlink { 0%,100%{transform:scaleY(1);} 40%,60%{transform:scaleY(0.1);} }

  /* dance — a fun shuffle-boogie */
  .cp-anim-dance .cp-whole { animation: kfDance 0.5s ease-in-out infinite; transform-origin:50% 100%; }
  .cp-anim-dance .cp-legs { animation: kfDanceLegs 0.25s ease-in-out infinite; }
  .cp-anim-dance .cp-body { animation: kfDanceBob 0.5s ease-in-out infinite; }
  @keyframes kfDance { 0%,100%{transform:rotate(-5deg) translateY(0);} 50%{transform:rotate(5deg) translateY(-2px);} }
  @keyframes kfDanceBob { 0%,100%{transform:scaleY(1);} 50%{transform:scaleY(1.03);} }
  @keyframes kfDanceLegs { 0%,100%{transform:translate(0,0);} 50%{transform:translate(3px,1.5px);} }

  /* ---- mood accessories ---- */
  .cp-sweat { animation: kfSweat 1s ease-in infinite; }
  @keyframes kfSweat { 0%{transform:translateY(0); opacity:0;} 30%{opacity:1;} 100%{transform:translateY(6px); opacity:0;} }
  .cp-starve { animation: kfStarve 1.4s ease-in-out infinite; transform-origin:50% 50%; }
  @keyframes kfStarve { 0%,100%{transform:scale(1); opacity:0.7;} 50%{transform:scale(1.12); opacity:1;} }
  .cp-hearts { animation: kfHearts 1.8s ease-out infinite; transform-origin:50% 50%; }
  @keyframes kfHearts { 0%{transform:scale(0.6) translateY(1px); opacity:0;} 20%{opacity:1;} 100%{transform:scale(1.15) translateY(-4px); opacity:0;} }

  /* smooth hand-off between whole-body animations */
  .cp-whole { transition: transform 0.15s ease, opacity 0.2s ease; }
`;

/* Minecraft-style icons for the pet HUD — a pixel steak (food) and a pixel
 * heart (love), inlined so no asset pipeline is needed. */
function SteakIcon({ size = 12 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 12" aria-hidden>
      <rect x="1" y="1" width="14" height="10" rx="2" fill="#a04d2d" />
      <rect x="3" y="3" width="10" height="2" fill="#d98a5a" />
      <rect x="3" y="7" width="10" height="2" fill="#7c3a22" />
      <rect x="5.5" y="0.5" width="5" height="1.5" rx="0.75" fill="#e8dcc0" />
    </svg>
  );
}

function HeartIcon({ size = 12 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 12 12" aria-hidden>
      <path
        d="M6 10 C2 7 1 4.5 2.4 2.9 C3.6 1.5 5.2 1.9 6 3.2 C6.8 1.9 8.4 1.5 9.6 2.9 C11 4.5 10 7 6 10 Z"
        fill="#e24c4c"
      />
    </svg>
  );
}

/** A tiny stat row: an icon, a bar, and a number — reused for love/food. */
function StatRow({
  icon,
  value,
  color,
  title,
}: {
  icon: React.ReactNode;
  value: number;
  color: string;
  title: string;
}) {
  return (
    <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }} title={title}>
      <span style={{ display: "inline-flex", alignItems: "center" }}>{icon}</span>
      <div
        style={{
          width: "100%",
          margin: "0 5px",
          height: 5,
          background: "var(--bg)",
          border: "1px solid var(--line)",
          borderRadius: 2,
        }}
      >
        <div
          style={{
            width: `${clip(value)}%`,
            height: "100%",
            background: color,
            borderRadius: 2,
            transition: "width 0.4s ease",
          }}
        />
      </div>
      <span style={{ fontSize: 8.5, fontVariantNumeric: "tabular-nums" }}>{clip(Math.round(value))}</span>
    </div>
  );
}

/** Feed button showing the steak + cooldown state ("ready" / "mm:ss"). */
function FeedCtl({
  ready,
  remainingSec,
  onClick,
}: {
  ready: boolean;
  remainingSec: number;
  onClick: () => void;
}) {
  const mm = Math.floor(remainingSec / 60);
  const ss = remainingSec % 60;
  const cd = `${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}`;
  return (
    <button
      className={ready ? "action" : "ghost"}
      onClick={onClick}
      disabled={!ready}
      title={ready ? "feed a steak (+8 love, -65 hunger)" : `steak ready in ${cd}`}
      style={{ fontSize: 9, padding: "2px 6px", display: "inline-flex", alignItems: "center", gap: 4 }}
    >
      <SteakIcon size={10} />
      {ready ? "feed" : `in ${cd}`}
    </button>
  );
}

export default function Tamagotchi() {
  const [m, setM] = useState<api.LiveMetrics | null>(null);
  const [open, setOpen] = useState(true);

  // --- pet state (persisted) --------------------------------------
  const [love, setLove] = useState(50);
  const [hunger, setHunger] = useState(0);
  const [lastFed, setLastFed] = useState(0);
  const [loaded, setLoaded] = useState(false);
  // re-render trigger so the feed-cooldown countdown ticks each second
  const [, setCdTick] = useState(0);

  // --- idle animation loop ----------------------------------------
  const [anim, setAnim] = useState<PetAnim>(IDLE);
  const [playing, setPlaying] = useState(false);
  const nextAt = useRef(0);
  const animTimer = useRef<number | null>(null);

  /** Tracks the last time the pet state was advanced, so hunger/love climb
   * against real elapsed time even while the app stays open. */
  const lastSeenRef = useRef(epochSecs());

  // telemetry polling (unchanged) + clamp to flicker-free seconds
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

  // tick the feed-cooldown display once a second
  useEffect(() => {
    const id = setInterval(() => setCdTick((t) => t + 1), 1000);
    return () => clearInterval(id);
  }, []);

  // load pet from settings DB once
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const [lv, hg, ls, lfed] = await Promise.all([
          api.settingsGet(PET_LOVE_KEY),
          api.settingsGet(PET_HUNGER_KEY),
          api.settingsGet(PET_LAST_SEEN_KEY),
          api.settingsGet(PET_LAST_FED_KEY),
        ]);
        if (!alive) return;
        const p = petFromRaw(lv, hg, ls, epochSecs(), lfed);
        setLove(p.love);
        setHunger(p.hunger);
        setLastFed(p.last_fed);
        lastSeenRef.current = p.last_seen;
        setLoaded(true);
      } catch {
        /* DB unavailable — start fresh */
        setLoaded(true);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // advance hunger/love on real wall-clock while open; persist on change
  useEffect(() => {
    if (!loaded) return;
    const id = setInterval(() => {
      const now = epochSecs();
      const p = advance(
        { love, hunger, last_seen: lastSeenRef.current, last_fed: lastFed },
        now,
      );
      lastSeenRef.current = now;
      setLove(p.love);
      setHunger(p.hunger);
    }, POLL_MS * 3);
    return () => clearInterval(id);
  }, [loaded, love, hunger, lastFed]);

  // persist whenever love/hunger change (throttled)
  const persistT = useRef<number | null>(null);
  useEffect(() => {
    if (!loaded) return;
    if (persistT.current) clearTimeout(persistT.current);
    persistT.current = window.setTimeout(() => {
      const now = epochSecs();
      void api.settingsSet(PET_LOVE_KEY, String(clip(love)), "pet love", "pet");
      void api.settingsSet(PET_HUNGER_KEY, String(clip(hunger)), "pet hunger", "pet");
      void api.settingsSet(PET_LAST_FED_KEY, String(lastFed), "pet feed cooldown", "pet");
      void api.settingsSet(PET_LAST_SEEN_KEY, String(now), "pet heartbeat", "pet");
    }, 600);
    return () => {
      if (persistT.current) clearTimeout(persistT.current);
    };
  }, [love, hunger, lastFed, loaded]);

  // --- idle animation scheduler ------------------------------------
  useEffect(() => {
    const step = () => {
      const now = performance.now();
      if (now >= nextAt.current) {
        const a = pickIdleAnim();
        setAnim(a);
        setPlaying(true);
        animTimer.current = window.setTimeout(() => setPlaying(false), a.ms * a.cycles);
        nextAt.current = now + a.ms * a.cycles + (1200 + Math.random() * 2600); // pause between
      }
    };
    // run step; reschedule a step on a short interval
    step();
    const t = window.setInterval(step, 250);
    return () => {
      clearInterval(t);
      if (animTimer.current) clearTimeout(animTimer.current);
    };
  }, []);

  // --- physical reactions (mood + pet face) -------------------------
  const vram = m ? pct(m.vram_used_mb, m.vram_total_mb) : 0;
  const mem = m ? pct(m.ram_used_mb, m.ram_total_mb) : 0;
  const load = m ? Math.max(m.gpu_util, vram, mem, m.cpu_pct) : 0;
  const mood: LoadMood =
    load >= 88 ? "hot" : load >= 65 ? "busy" : load >= 35 ? "focus" : "calm";

  const face: PetFace = loaded
    ? faceFrom({ love, hunger, last_seen: epochSecs(), last_fed: lastFed }, load)
    : "content";
  const starving = loaded && hunger >= 90;
  const feedReady = canFeed({ love, hunger, last_seen: epochSecs(), last_fed: lastFed });
  // seconds until the feed cooldown clears (0 when ready); re-renders tick.
  const feedRemainingSec = Math.max(
    0,
    Math.ceil((lastFed * 1000 + 30 * 60 * 1000 - Date.now()) / 1000),
  );

  const tint =
    starving ? "var(--oom)"
    : mood === "hot" ? "var(--oom)"
    : mood === "busy" ? "var(--warn)"
    : mood === "focus" ? "var(--cyan)"
    : "var(--pass)";

  const onFeed = () => {
    if (!feedReady) return; // cooldown — no grind
    const now = epochSecs();
    const f = feed({ love, hunger, last_seen: now, last_fed: lastFed }); // feed sets last_fed
    lastSeenRef.current = now;
    setLove(f.love);
    setHunger(f.hunger);
    setLastFed(f.last_fed);
  };
  const onPet = () => {
    const now = epochSecs();
    const f = pet({ love, hunger, last_seen: now, last_fed: lastFed }); // pet advances internally
    lastSeenRef.current = now;
    setLove(f.love);
    setHunger(f.hunger);
  };

  const meters = m
    ? [
        { label: "VRAM", v: vram, r: m.vram_used_mb, t: m.vram_total_mb },
        { label: "GPU", v: m.gpu_util, r: m.gpu_util, t: 100 },
        { label: "RAM", v: mem, r: m.ram_used_mb, t: m.ram_total_mb },
        { label: "CPU", v: m.cpu_pct, r: m.cpu_pct, t: 100 },
      ]
    : [];

  const title = faceLabel(face, love, hunger);

  const bodyClass = `cp-body`;
  const wholeClass = playing ? animClass(anim.id) : "cp-idle";

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
      <style>{keyframes}</style>

      {/* pet body — whole-group animation carries walk/flip/spin/bounce */}
      <div className="cp-stage">
        <button
          onClick={() => setOpen((o) => !o)}
          title={title}
          style={{ background: "none", border: "none", padding: 0, cursor: "pointer", lineHeight: 0, display: "block" }}
        >
          <svg className={wholeClass} viewBox="0 0 72 60" width={open ? 44 : 34} height={open ? 37 : 29}>
            <g className="cp-whole">
              <ellipse className={bodyClass} cx="36" cy="40" rx="27" ry="17" fill={tint} opacity="0.9" />
              {/* feet */}
              <g className="cp-legs">
                <path d="M16 52 q2 5 8 0" fill="none" stroke={tint} strokeWidth="3.5" strokeLinecap="round" />
                <path d="M56 52 q-2 5 -8 0" fill="none" stroke={tint} strokeWidth="3.5" strokeLinecap="round" />
              </g>
              {/* eyes + pupils (dart) */}
              <g className="cp-eyes">
                <circle cx="26" cy="35" r="6" fill="#fff" />
                <circle cx="46" cy="35" r="6" fill="#fff" />
                <g className="cp-pupils">
                  <circle cx="28" cy="37" r="2.8" fill={moodShade(mood)} />
                  <circle cx="48" cy="37" r="2.8" fill={moodShade(mood)} />
                </g>
              </g>
              {/* mouth: shape driven by the pet face */}
              <g className="cp-mouth">
                <path
                  d={face === "starving" || face === "hangry" ? "M33 49 q3 -4 6 0" : face === "blissful" ? "M33 45 q3 4 6 0" : "M33 47 q3 3 6 0"}
                  fill="none"
                  stroke="#111"
                  strokeWidth="2"
                  strokeLinecap="round"
                />
              </g>
              {/* cheeks */}
              <g className="cp-cheeks">
                <ellipse cx="20" cy="43" rx="3.5" ry="2" fill="var(--oom)" opacity="0.35" />
                <ellipse cx="52" cy="43" rx="3.5" ry="2" fill="var(--oom)" opacity="0.35" />
              </g>
            </g>
            {/* temperature sweat */}
            {mood === "hot" && (
              <g className="cp-sweat">
                <path d="M16 24 q4 6 0 8 q-4 -2 0 -8" fill="var(--cyan)" />
                <path d="M58 20 q4 6 0 8 q-4 -2 0 -8" fill="var(--cyan)" />
              </g>
            )}
            {/* hunger bubbles / hearts */}
            {starving && (
              <g className="cp-starve">
                <path d="M20 18 q-2 -6 -6 -6 q-5 0 -2 8 l4 5 l-4 5" fill="none" stroke="var(--warn)" strokeWidth="2" />
              </g>
            )}
            {face === "blissful" && (
              <g className="cp-hearts">
                <path d="M20 18 a2.2 2.2 0 1 0 4.4 0 a4.4 4.4 0 0 0 -4.4 -4.4 a4.4 4.4 0 0 0 -4.4 4.4 a2.2 2.2 0 1 0 4.4 0" fill="var(--oom)" />
              </g>
            )}
          </svg>
        </button>
      </div>

      {/* ---- PET STATS (separate from system telemetry below) ---- */}
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 4,
          width: "100%",
          padding: "4px 8px 6px",
          borderTop: "1px dashed var(--line)",
        }}
      >
        <div style={{ fontSize: 8, letterSpacing: 1, opacity: 0.7, textAlign: "center" }}>PET</div>
        <StatRow icon={<HeartIcon size={11} />} value={love} color="var(--oom)" title={`love ${Math.round(love)}/100`} />
        <StatRow icon={<SteakIcon size={11} />} value={100 - hunger} color="var(--warn)" title={`food ${Math.round(100 - hunger)}/100`} />
        <div className="row" style={{ gap: 5, justifyContent: "center", marginTop: 2 }}>
          <FeedCtl ready={feedReady} remainingSec={feedRemainingSec} onClick={onFeed} />
          <button className="ghost" onClick={onPet} title={`cuddle (+${6} love)`} style={{ fontSize: 9, padding: "2px 6px" }}>
            <HeartIcon size={9} /> pet
          </button>
        </div>
        <div className="dim" style={{ fontSize: 8, textAlign: "center" }}>{faceLabel(face, love, hunger)}</div>
      </div>

      {/* ---- SYSTEM TELEMETRY (always on screen, separate from PET) ---- */}
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 4,
          width: "100%",
          padding: "4px 8px 3px",
          borderTop: "1px solid var(--line)",
        }}
      >
        {meters.map((s) => (
          <div key={s.label} title={`${s.r} / ${s.t}`}>
            <div style={{ display: "flex", justifyContent: "space-between", fontSize: 9, letterSpacing: 0.5 }}>
              <span className="dim">{s.label}</span>
              <span style={{ fontVariantNumeric: "tabular-nums" }}>{s.v}%</span>
            </div>
            <div style={{ width: "100%", height: 4, background: "var(--bg)", border: "1px solid var(--line)", borderRadius: 3, marginTop: 2 }}>
              <div
                style={{ width: `${Math.max(2, s.v)}%`, height: "100%", background: tint, borderRadius: 2 }}
              />
            </div>
          </div>
        ))}
        {m && (
          <div className="dim mono" style={{ fontSize: 8.5, textAlign: "center", paddingTop: 2 }}>
            {(m.vram_used_mb / 1024).toFixed(1)}G VRAM used
          </div>
        )}
      </div>
    </div>
  );
}
