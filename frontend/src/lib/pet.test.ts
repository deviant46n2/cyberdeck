import { describe, expect, it } from "vitest";
import {
  advance,
  canFeed,
  faceFrom,
  feed,
  FEED_COOLDOWN_MS,
  freshPet,
  pet,
  petFromRaw,
} from "./pet";
import { IDLE_ANIMS, pickIdleAnim } from "./pet-anim";

const T0 = 1_755_500_000;

describe("pet basics", () => {
  it("freshPet starts content: mid love, no hunger", () => {
    const p = freshPet(T0);
    expect(p.love).toBe(50);
    expect(p.hunger).toBe(0);
    expect(p.last_seen).toBe(T0);
  });

  it("advance raises hunger with real elapsed minutes and updates last_seen", () => {
    const p = freshPet(T0);
    // ~7h later hunger clears the neglect line but stays under the cap
    const p2 = advance(p, T0 + 7 * 60 * 60);
    expect(p2.hunger).toBeGreaterThan(60);
    expect(p2.hunger).toBeLessThan(100);
    expect(p2.last_seen).toBe(T0 + 7 * 60 * 60);
    // prolonged neglect starts eroding love, but not to zero in one stretch
    expect(p2.love).toBeLessThan(50);
    expect(p2.love).toBeGreaterThan(0);
  });

  it("advance erodes love only after hunger crosses the neglect line", () => {
    const p = freshPet(T0);
    // 30 min -> hunger ~5, well under 60 -> no love loss
    const short = advance(p, T0 + 30 * 60);
    expect(short.love).toBe(50);
    // 12h of total neglect -> hunger capped at 100, love eroded
    const long = advance(p, T0 + 12 * 60 * 60);
    expect(long.love).toBeLessThan(50);
  });

  it("clamps love and hunger to [0,100]", () => {
    const p = freshPet(T0);
    // feed from stuffed
    const starved = { ...p, hunger: 100, love: 100 };
    expect(starved.hunger).toBe(100);
  });
});

describe("pet interactions", () => {
  it("feed drops hunger and bumps love", () => {
    const p = { ...freshPet(T0), hunger: 80, love: 40 };
    const f = feed(p, T0 + 10); // 10s later
    expect(f.hunger).toBeLessThan(80);
    expect(f.love).toBeGreaterThan(40);
  });

  it("feed never lets love or hunger escape [0,100]", () => {
    const p = { ...freshPet(T0), hunger: 5, love: 99 };
    const f = feed(p, T0);
    expect(f.hunger).toBeGreaterThanOrEqual(0);
    expect(f.love).toBeLessThanOrEqual(100);
  });

  it("pet bumps love without meaningful hunger change in the same instant", () => {
    const p = { ...freshPet(T0), hunger: 30, love: 20 };
    const p2 = pet(p, T0 + 10);
    expect(p2.love).toBeGreaterThan(20);
    // hunger only rises by the ~10s elapsed (sub-minute), not by a full point
    expect(p2.hunger).toBeGreaterThanOrEqual(30);
    expect(p2.hunger).toBeLessThan(31);
  });

  it("feed records last_fed in seconds so the cooldown gates re-feeding", () => {
    const never = freshPet(T0); // last_fed = 0
    expect(canFeed(never, T0 * 1000)).toBe(true);
    const f = feed(never, T0);
    expect(f.last_fed).toBe(T0);
    // right after feeding -> still on cooldown
    expect(canFeed(f, T0 * 1000 + 1000)).toBe(false);
    // after the cooldown window -> ready again
    expect(canFeed(f, (T0 + FEED_COOLDOWN_MS / 1000) * 1000)).toBe(true);
  });
});

describe("petFromRaw", () => {
  it("rebuilds from settings values, stripping JSON quotes and clamping", () => {
    const p = petFromRaw('"70"', '"45"', '"1755500100"', T0);
    expect(p.love).toBe(70);
    expect(p.hunger).toBe(45);
    expect(p.last_seen).toBe(1_755_500_100);
  });
  it("falls back to a fresh pet for missing/invalid fields", () => {
    const p = petFromRaw(null, "bogus", undefined, T0);
    expect(p.love).toBe(50);
    expect(p.hunger).toBe(0);
    expect(p.last_seen).toBe(T0);
  });
  it("clamps out-of-range persisted values", () => {
    const p = petFromRaw('"150"', '"-20"', null, T0);
    expect(p.love).toBe(100);
    expect(p.hunger).toBe(0);
  });
});

describe("faceFrom", () => {
  const base = { love: 50, hunger: 0, last_seen: T0, last_fed: T0 };
  it("starves at very high hunger", () => {
    expect(faceFrom({ ...base, hunger: 95 }, 0)).toBe("starving");
  });
  it("is hangry at high hunger or heavy load", () => {
    expect(faceFrom({ ...base, hunger: 75 }, 0)).toBe("hangry");
    expect(faceFrom({ ...base, hunger: 20 }, 92)).toBe("hangry");
  });
  it("is blissful at high love", () => {
    expect(faceFrom({ ...base, love: 90 }, 10)).toBe("blissful");
  });
  it("is loved at mid-high love", () => {
    expect(faceFrom({ ...base, love: 65 }, 10)).toBe("loved");
  });
  it("is content otherwise", () => {
    expect(faceFrom(base, 10)).toBe("content");
  });
});

describe("idle animation scheduler", () => {
  it("pickIdleAnim always returns a defined animation", () => {
    for (let i = 0; i < 50; i++) {
      const a = pickIdleAnim(() => i / 100);
      expect(a).toBeDefined();
    }
  });
  it("has a non-empty, unique-id catalogue", () => {
    expect(IDLE_ANIMS.length).toBeGreaterThan(5);
    const ids = new Set(IDLE_ANIMS.map((a) => a.id));
    expect(ids.size).toBe(IDLE_ANIMS.length);
  });
  it("seeded rolls are reproducible", () => {
    const a = pickIdleAnim(() => 0.0);
    const b = pickIdleAnim(() => 0.0);
    expect(a.id).toBe(b.id);
  });
});
