// Single-flight bring-up (LOAD) and headless TEST state, backed by the
// bringup-* Tauri events shared by deck-tauri::bringup_start and
// deck-tauri::test_model_start. The Bringup drawer mounted in App is the only
// consumer; VAULT / DOWNLOADS buttons call startBringup() / startTest().
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

export type BrMode = "load" | "test";

export interface BrState {
  running: boolean;
  mode: BrMode;
  phase: string; // derive | verify | apply | bench | done | error | idle
  lines: string[];
  result: api.BringupResult | null;
  /** The derived profile — present after derive so the tweak panel can work
   * even when verification or apply fails. */
  profile: api.Profile | null;
}

let state: BrState = {
  running: false,
  mode: "load",
  phase: "idle",
  lines: [],
  result: null,
  profile: null,
};

let version = 0;
const listeners = new Set<() => void>();

function bump() {
  version++;
  listeners.forEach((l) => l());
}

export function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getSnapshot(): BrState {
  return state;
}

function pushLine(text: string) {
  // keep the log bounded — this drawer is a status strip, not a terminal
  state = { ...state, lines: [...state.lines, text].slice(-9) };
}

let initialized = false;
let watchdog: ReturnType<typeof setTimeout> | null = null;

function armWatchdog() {
  if (watchdog) clearTimeout(watchdog);
  watchdog = setTimeout(() => {
    if (state.running) {
      state = {
        ...state,
        running: false,
        phase: "error",
        lines: [...state.lines, "[timeout] bring-up stalled — no progress in 180s, check logs"].slice(-9),
        result: { ok: false, summary: "bring-up timed out after 180s with no progress", name: "", port: 0, ctx: 0, tps: null, fit: null },
      };
      bump();
    }
  }, 180_000);
}

function clearWatchdog() {
  if (watchdog) { clearTimeout(watchdog); watchdog = null; }
}

/** Idempotently attach the backing listeners. */
export function init(): void {
  if (initialized || typeof window === "undefined") return;
  initialized = true;

  listen<{ phase: string }>("bringup-phase", ({ payload }) => {
    if (payload.phase === "done") clearWatchdog(); else armWatchdog();
    state =
      payload.phase === "done"
        ? { ...state, running: false, phase: "done" }
        : { ...state, running: true, phase: payload.phase };
    bump();
  });

  listen<{ text: string }>("bringup-line", ({ payload }) => {
    pushLine(payload.text);
    bump();
  });

  listen<api.Profile>("bringup-profile", ({ payload }) => {
    state = { ...state, profile: payload };
    bump();
  });

  listen<api.BringupResult>("bringup-result", ({ payload }) => {
    clearWatchdog();
    state = { ...state, result: payload };
    if (!payload.ok) pushLine(`[error] ${payload.summary}`);
    bump();
  });
}

/** Kick off a one-click load. Errors surface as an immediate failed run. */
export async function startBringup(modelPath: string, engine: string): Promise<void> {
  init();
  armWatchdog();
  state = {
    running: true,
    mode: "load",
    phase: "derive",
    lines: [`[load] ${modelPath} → ${engine}`],
    result: null,
    profile: null,
  };
  bump();
  try {
    await api.bringupStart(modelPath, engine);
  } catch (e) {
    clearWatchdog();
    const msg = String(e);
    state = {
      ...state,
      running: false,
      phase: msg.includes("already running") ? state.phase : "error",
      lines: [...state.lines, `[reject] ${msg}`].slice(-9),
      result: msg.includes("already running")
        ? null
        : { ok: false, summary: msg, name: "", port: 0, ctx: 0, tps: null, fit: null },
    };
    bump();
  }
}

/** Headless TEST — derive + verify on the test port; the live service is never
 * touched and nothing is installed. Result marks itself "NOT applied". */
export async function startTest(modelPath: string, engine: string): Promise<void> {
  init();
  armWatchdog();
  state = {
    running: true,
    mode: "test",
    phase: "derive",
    lines: [`[test] ${modelPath} → ${engine} (headless, not applied)`],
    result: null,
    profile: null,
  };
  bump();
  try {
    await api.testModelStart(modelPath, engine);
  } catch (e) {
    clearWatchdog();
    const msg = String(e);
    state = {
      ...state,
      running: false,
      phase: msg.includes("already running") ? state.phase : "error",
      lines: [...state.lines, `[reject] ${msg}`].slice(-9),
      result: msg.includes("already running")
        ? null
        : { ok: false, summary: msg, name: "", port: 0, ctx: 0, tps: null, fit: null },
    };
    bump();
  }
}

/** Apply the already-verified profile from a TEST run (skip derive+verify)
 * and bench+record. Shares the test mode so the panel shows apply → bench. */
export async function startApplyCached(): Promise<void> {
  init();
  const profile = state.profile;
  const fit = state.result?.fit ?? null;
  if (!profile) {
    state = { ...state, running: false, phase: "error", lines: ["[apply] no verified profile — run TEST first"].slice(-9) };
    bump();
    return;
  }
  state = { ...state, running: true, phase: "apply", lines: ["[apply] applying verified profile…"] };
  bump();
  try {
    await api.testApply(profile, fit);
  } catch (e) {
    const msg = String(e);
    state = {
      ...state, running: false, phase: "error",
      lines: [...state.lines, `[reject] ${msg}`].slice(-9),
      result: msg.includes("already running") ? null : { ok: false, summary: msg, name: "", port: 0, ctx: 0, tps: null, fit: null },
    };
    bump();
  }
}

/** Clear the finished/failed card. No-op while a run is in flight. */
export function dismiss(): void {
  if (state.running) return;
  state = { running: false, mode: "load", phase: "idle", lines: [], result: null, profile: null };
  bump();
}

export async function forceReset(): Promise<void> {
  clearWatchdog();
  try { await api.bringupReset(); } catch {}
  state = { running: false, mode: "load", phase: "idle", lines: [], result: null, profile: null };
  bump();
}

/**
 * Re-verify with tweaked parameters. Updates the state with the result and
 * keeps the profile so further tweaks can stack.
 */
export async function tweakWith(
  profile: api.Profile,
  tweaks: { ctx?: number; kvBytes?: number; offload?: boolean; ngl?: number },
): Promise<void> {
  init();
  state = { ...state, running: true, phase: "verify", lines: [...state.lines, "[tweak] verifying…"].slice(-9) };
  bump();
  try {
    const r = await api.tweakProfile({ profile, ...tweaks });
    state = {
      ...state,
      running: false,
      phase: "done",
      result: { ok: r.ok, summary: r.summary, name: profile.name, port: profile.port, ctx: r.ctx, tps: r.tps, fit: null },
      profile,
    };
    pushLine(`[tweak] ${r.summary}`);
    bump();
  } catch (e) {
    const msg = String(e);
    state = { ...state, running: false, phase: "done" };
    pushLine(`[tweak] ${msg}`);
    bump();
  }
}
