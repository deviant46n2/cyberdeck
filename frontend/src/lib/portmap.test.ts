import { describe, expect, it } from "vitest";
import type { BenchRow } from "../api";
import { latestBySlot, slotKey, sortSlots } from "./portmap";

const row = (over: Partial<BenchRow>): BenchRow => ({
  id: 1,
  engine: "llamacpp",
  host: "127.0.0.1",
  port: 18000,
  model: "qwen3.8-27b",
  ctx: 32768,
  tps: 49.3,
  at: 100,
  ...over,
});

describe("slotKey", () => {
  it("keys on engine and port", () => {
    expect(slotKey("llamacpp", 18000)).toBe("llamacpp:18000");
  });
});

describe("latestBySlot", () => {
  it("keeps the newest reading per slot", () => {
    const m = latestBySlot([
      row({ id: 1, at: 100, tps: 49.3 }),
      row({ id: 2, at: 200, tps: 51.0 }),
      row({ id: 3, at: 150, tps: 2.0 }),
    ]);
    expect(m.get("llamacpp:18000")?.tps).toBe(51.0);
  });

  it("does not cross slots on the same engine", () => {
    const m = latestBySlot([
      row({ id: 1, port: 18000, tps: 49.3, at: 100 }),
      row({ id: 2, port: 18999, tps: 3.0, at: 200 }),
    ]);
    expect(m.get("llamacpp:18000")?.tps).toBe(49.3);
    expect(m.get("llamacpp:18999")?.tps).toBe(3.0);
  });

  it("returns an empty map for no rows", () => {
    expect(latestBySlot([]).size).toBe(0);
  });
});

describe("sortSlots", () => {
  it("orders up before starting before down", () => {
    const out = sortSlots([
      { engine: "ollama", display: "Ollama", port: 11434, profile: null, resident: false, state: "down", fit_verdict: null },
      { engine: "freetoken", display: "FreeToken", port: 1919, profile: "ft-x", resident: true, state: "up", fit_verdict: null },
      { engine: "llamacpp", display: "llama.cpp", port: 18000, profile: "q", resident: false, state: "starting", fit_verdict: null },
    ]);
    expect(out.map((s) => s.engine)).toEqual(["freetoken", "llamacpp", "ollama"]);
  });
});
