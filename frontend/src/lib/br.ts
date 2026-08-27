// Single-flight bring-up (LOAD) state, backed by bringup-* Tauri events.
// The Bringup drawer mounted in App is the only consumer; VAULT buttons call
// startBringup().
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

export interface BrState {
  running: boolean;
  phase: string; // derive | verify | apply | bench | done | error | idle
  lines: string[];
  result: api.BringupResult | null;
  /** The derived profile — present after derive so the tweak panel can work
   * even when verification or apply fails. */
  profile: api.Profile | null;
}

let state: BrState = {
  running: false,
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

/** Idempotently attach the backing listeners. */
export function init(): void {
  if (initialized || typeof window === "undefined") return;
  initialized = true;

  listen<{ phase: string }>("bringup-phase", ({ payload }) => {
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
    state = { ...state, result: payload };
    if (!payload.ok) pushLine(`[error] ${payload.summary}`);
    bump();
  });
}

/** Kick off a one-click load. Errors surface as an immediate failed run. */
export async function startBringup(modelPath: string, engine: string): Promise<void> {
  init();
  state = { running: true, phase: "derive", lines: [`[load] ${modelPath} → ${engine}`], result: null, profile: null };
  bump();
  try {
    await api.bringupStart(modelPath, engine);
  } catch (e) {
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

/** Clear the finished/failed card. No-op while a run is in flight. */
export function dismiss(): void {
  if (state.running) return;
  state = { running: false, phase: "idle", lines: [], result: null, profile: null };
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
