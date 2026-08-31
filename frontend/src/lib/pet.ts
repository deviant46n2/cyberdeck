/**
 * Tamagotchi pet state mechanics — the "living" half of the sidebar guy.
 *
 * Pure + time-based so the pet feels real: hunger rises and love decays with
 * actual wall-clock elapsed since the last interaction, not just on-tick.
 * State is persisted through the settings DB (double-quoted JSON, same
 * convention as `feeds.last_seen`), so the pet survives app restarts and is
 * "two-door" in spirit.
 */

export interface PetState {
  /** 0..100 — how loved/cared-for he feels. Feeding/petting raise it. */
  love: number;
  /** 0..100 — 0 = stuffed, 100 = starving. Rises slowly with real time
   * (~10 pts/hour → one steak lasts ~6h, so 2-3 feeds a day keeps him fed). */
  hunger: number;
  /** Epoch seconds of the last persisted interaction / heartbeat. */
  last_seen: number;
  /** Epoch seconds of the last time he was fed. Drives the feed cooldown;
   * `0` = never fed yet (feed allowed immediately). */
  last_fed: number;
}

/** Keys in the settings DB that hold pet state. */
export const PET_LOVE_KEY = "pet.love";
export const PET_HUNGER_KEY = "pet.hunger";
export const PET_LAST_SEEN_KEY = "pet.last_seen";
export const PET_LAST_FED_KEY = "pet.last_fed";

/**
 * Balance: hunger rises HUNGER_PER_MIN so one full steak (FEED_FILL) covers
 * ~6 waking hours — you only reach for food 2-3x a day. FEED_COOLDOWN_MS
 * makes feeding deliberate, not a button mash.
 */
export const HUNGER_PER_MIN = 10 / 60; // ~0.167 → 0→100 over ~10h
export const FEED_FILL = 65; // hunger points one steak restores
export const FEED_LOVE = 8; // love bump per steak
export const PET_LOVE = 6; // love bump per cuddle
export const FEED_COOLDOWN_MS = 30 * 60 * 1000; // 30 min between feeds
/** Love only erodes once hunger climbs past this neglect line. */
export const NEGLECT_HUNGER = 60;

export const MAX = 100;
export const MIN = 0;

const clamp = (v: number) => Math.min(MAX, Math.max(MIN, v));

export function freshPet(nowSecs = epochSecs()) {
  return { love: 50, hunger: 0, last_seen: nowSecs, last_fed: 0 };
}

export function epochSecs(): number {
  return Math.floor(Date.now() / 1000);
}

/** Strip the JSON quotes `settings_get` wraps around a persisted number. */
function num(raw: string | null | undefined, fallback: number): number {
  if (!raw) return fallback;
  const n = Number(raw.replace(/^"|"$/g, ""));
  return Number.isFinite(n) ? n : fallback;
}

/** Rebuild pet state from the raw settings-DB values. Missing/invalid fields
 * fall back to a freshly-born pet. */
export function petFromRaw(
  loveRaw: string | null | undefined,
  hungerRaw: string | null | undefined,
  lastSeenRaw: string | null | undefined,
  nowSecs = epochSecs(),
  lastFedRaw?: string | null | undefined,
): PetState {
  const love = clamp(num(loveRaw, 50));
  const hunger = clamp(num(hungerRaw, 0));
  const last = num(lastSeenRaw, nowSecs);
  const lastFed = lastFedRaw != null ? num(lastFedRaw, 0) : 0;
  return { love, hunger, last_seen: last, last_fed: lastFed };
}

/**
 * Advance the pet by real elapsed time since `last_seen`.
 * Hunger rises ~10 pts/hour (a couple feeds a day keeps him topped up); love
 * erodes slowly but only after hunger climbs past the neglect line.
 */
export function advance(p: PetState, nowSecs = epochSecs()): PetState {
  const elapsed = Math.max(0, nowSecs - p.last_seen);
  const mins = elapsed / 60;
  const hunger = clamp(p.hunger + mins * HUNGER_PER_MIN);
  // Love holds while fed; erodes past the neglect line.
  const neglect = Math.max(0, hunger - NEGLECT_HUNGER) / (100 - NEGLECT_HUNGER);
  const love = clamp(p.love - neglect * mins * 0.02);
  return { love, hunger, last_seen: nowSecs, last_fed: p.last_fed };
}

/** True when the pet can be fed again (cooldown elapsed, or never fed). */
export function canFeed(p: PetState, nowMs = Date.now()): boolean {
  if (p.last_fed === 0) return true;
  return nowMs - p.last_fed * 1000 >= FEED_COOLDOWN_MS;
}

/** Feed: hunger drops a lot, love bumps, and the cooldown resets. Callers
 * should gate on `canFeed` for UX; feeding during cooldown still returns a
 * valid state (it just doesn't spam-raise love). */
export function feed(p: PetState, nowSecs = epochSecs()): PetState {
  const a = advance(p, nowSecs);
  return {
    ...a,
    hunger: clamp(a.hunger - FEED_FILL),
    love: clamp(a.love + FEED_LOVE),
    last_fed: nowSecs,
  };
}

/** Pet / cuddle: pure love bump, no cost, slight cooldown handled by caller. */
export function pet(p: PetState, nowSecs = epochSecs()): PetState {
  const a = advance(p, nowSecs);
  return { ...a, love: clamp(a.love + PET_LOVE) };
}

/** Human-ish face of the pet from combined load + state. */
export type PetFace =
  | "starving"
  | "hangry"
  | "content"
  | "loved"
  | "blissful";

export function faceFrom(p: PetState, load: number): PetFace {
  if (p.hunger >= 90) return "starving";
  if (p.hunger >= 65) return "hangry";
  if (p.love >= 85) return "blissful";
  if (p.love >= 60) return "loved";
  if ((load ?? 0) >= 88) return "hangry"; // overload can read as grumpy
  return "content";
}
