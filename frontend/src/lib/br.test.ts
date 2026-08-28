// Unit tests for the single-flight bringup/TEST state store in lib/br.ts.
// Same harness shape as dl.test.ts: a hoisted event bus that captures the
// tauri listen callbacks so tests can drive bringup-* events, plus a window
// stub so init() attaches them.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as api from "../api";

const bus = vi.hoisted(() => new Map<string, (e: { payload: unknown }) => void>());

vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (e: { payload: unknown }) => void) => {
    bus.set(event, cb);
    return Promise.resolve(() => bus.delete(event));
  },
}));

const apiMock = vi.hoisted(() => ({
  bringupStart: vi.fn<() => Promise<void>>(() => Promise.resolve()),
  testModelStart: vi.fn<() => Promise<void>>(() => Promise.resolve()),
  tweakProfile: vi.fn<() => Promise<unknown>>(() => Promise.resolve({ ok: true, summary: "ok", ctx: 8192, tps: 12.3 })),
}));

vi.mock("../api", () => apiMock);

type BrModule = typeof import("./br");

async function freshStore(): Promise<BrModule> {
  vi.resetModules();
  (globalThis as { window?: unknown }).window = globalThis;
  const br = (await import("./br")) as BrModule;
  br.init();
  return br;
}

function emit(event: string, payload: unknown) {
  const cb = bus.get(event);
  expect(cb).toBeDefined();
  cb?.({ payload });
}

const flush = () => new Promise((r) => setTimeout(r, 0));

function fakeProfile(name: string): api.Profile {
  return {
    name,
    engine: "LlamaCpp",
    bin: "/usr/bin/llama-server",
    model: "~/models/a.gguf",
    alias: name,
    host: "127.0.0.1",
    port: 18000,
    metrics: true,
    ctx_size: 2048,
    ctx_ladder: [2048],
    n_gpu_layers: 128,
    ubatch_size: 512,
    flash_attn: false,
    kv_cache_type_k: null,
    kv_cache_type_v: null,
    load_mode: null,
    spec_type: null,
    draft_model: null,
    temperature: 0.8,
    top_p: 0.95,
    top_k: 40,
    parallel: 1,
    reasoning: null,
    reasoning_format: null,
    reasoning_effort: null,
    reasoning_budget: null,
    ft_backend: null,
    ft_moe_cache_size: null,
    mem_max_mb: null,
    mem_swap_max_mb: null,
  };
}

describe("br.ts bringup/bringup state store", () => {
  beforeEach(() => {
    bus.clear();
    apiMock.bringupStart.mockClear();
    apiMock.testModelStart.mockClear();
    apiMock.tweakProfile.mockClear();
    apiMock.bringupStart.mockImplementation(() => Promise.resolve());
    apiMock.testModelStart.mockImplementation(() => Promise.resolve());
    apiMock.tweakProfile.mockImplementation(() =>
      Promise.resolve({ ok: true, summary: "ok", ctx: 8192, tps: 12.3 }),
    );
  });

  it("startBringup sets a running load run and calls the backend", async () => {
    const br = await freshStore();
    await br.startBringup("~/models/a.gguf", "llamacpp");
    const s = br.getSnapshot();
    expect(s.running).toBe(true);
    expect(s.mode).toBe("load");
    expect(s.phase).toBe("derive");
    expect(s.lines[0]).toContain("→ llamacpp");
    expect(apiMock.bringupStart).toHaveBeenCalledWith("~/models/a.gguf", "llamacpp");
  });

  it("startTest runs headless with test mode", async () => {
    const br = await freshStore();
    await br.startTest("~/models/a.gguf", "freetoken");
    const s = br.getSnapshot();
    expect(s.running).toBe(true);
    expect(s.mode).toBe("test");
    expect(s.lines[0]).toContain("not applied");
    expect(apiMock.testModelStart).toHaveBeenCalledWith("~/models/a.gguf", "freetoken");
  });

  it("a hard reject surfaces as a failed run with result, preserving phase", async () => {
    const br = await freshStore();
    apiMock.bringupStart.mockImplementation(() => Promise.reject(new Error("no llamacpp binary")));
    await br.startBringup("~/models/a.gguf", "llamacpp");
    const s = br.getSnapshot();
    expect(s.running).toBe(false);
    expect(s.phase).toBe("error");
    expect(s.result?.ok).toBe(false);
    expect(s.result?.summary).toContain("no llamacpp binary");
  });

  it("an 'already running' reject keeps the phase and clears result", async () => {
    const br = await freshStore();
    apiMock.testModelStart.mockImplementation(() =>
      Promise.reject(new Error("a bring-up is already running")),
    );
    await br.startTest("~/models/a.gguf", "llamacpp");
    const s = br.getSnapshot();
    expect(s.running).toBe(false);
    expect(s.phase).toBe("derive");
    expect(s.result).toBeNull();
  });

  it("events drive phase, lines, profile, and result; a failed result logs an error line", async () => {
    const br = await freshStore();
    await br.startBringup("~/models/a.gguf", "llamacpp");
    emit("bringup-phase", { phase: "verify" });
    expect(br.getSnapshot().phase).toBe("verify");
    emit("bringup-line", { text: "[verify] loading on :18999" });
    emit("bringup-line", { text: "[verify] RUNNING" });
    const lines = br.getSnapshot().lines;
    expect(lines[lines.length - 1]).toBe("[verify] RUNNING");
    emit("bringup-profile", fakeProfile("qwen"));
    expect(br.getSnapshot().profile?.name).toBe("qwen");
    emit("bringup-result", { ok: true, summary: "TEST OK", name: "qwen", port: 18999, ctx: 2048, tps: 3.4, fit: null });
    let s = br.getSnapshot();
    expect(s.result?.ok).toBe(true);
    expect(s.phase).toBe("verify"); // result alone doesn't change phase
    expect(s.running).toBe(true);
    emit("bringup-phase", { phase: "done" });
    s = br.getSnapshot();
    expect(s.running).toBe(false);
    expect(s.phase).toBe("done");
  });

  it("the line log stays bounded to 9 entries", async () => {
    const br = await freshStore();
    for (let i = 0; i < 15; i++) emit("bringup-line", { text: `line ${i}` });
    expect(br.getSnapshot().lines).toHaveLength(9);
    expect(br.getSnapshot().lines[0]).toBe("line 6");
  });

  it("dismiss clears a finished run but is a no-op while running", async () => {
    const br = await freshStore();
    await br.startTest("~/models/a.gguf", "llamacpp");
    br.dismiss();
    expect(br.getSnapshot().running).toBe(true);
    emit("bringup-phase", { phase: "done" });
    br.dismiss();
    const s = br.getSnapshot();
    expect(s.phase).toBe("idle");
    expect(s.lines).toHaveLength(0);
    expect(s.result).toBeNull();
  });

  it("tweakWith reports a successful re-verify", async () => {
    const br = await freshStore();
    const profile = fakeProfile("qwen");
    await br.tweakWith(profile, { ctx: 4096 });
    const s = br.getSnapshot();
    expect(s.running).toBe(false);
    expect(s.phase).toBe("done");
    expect(s.result?.ctx).toBe(8192);
    expect(s.result?.tps).toBe(12.3);
    expect(apiMock.tweakProfile).toHaveBeenCalledWith({ profile, ctx: 4096 });
  });

  it("tweakWith reports a failure without dying", async () => {
    const br = await freshStore();
    apiMock.tweakProfile.mockImplementation(() => Promise.reject(new Error("OOM")));
    await br.tweakWith(fakeProfile("qwen"), {});
    const s = br.getSnapshot();
    expect(s.running).toBe(false);
    expect(s.phase).toBe("done");
    expect(s.lines[s.lines.length - 1]).toContain("OOM");
  });
});