// Global background-download store. Backed by the dl-* Tauri events emitted
// by deck-tauri's job registry, visible from every view via the Downloads
// drawer mounted in App.
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

export type DlStatus = "active" | "done" | "error";

export interface DlEntry {
  key: string;
  name: string;
  total: number;
  done: number;
  /** bytes/sec smoothed from event deltas; 0 until measurable */
  speed: number;
  startedAt: number;
  status: DlStatus;
  err?: string;
  lastTickMs?: number;
}

interface EvPayload {
  key: string;
  repo_id: string;
  rfilename: string;
  done?: number;
  total?: number;
  path?: string;
  error?: string;
}

const MAX_ACTIVE = 2;

const entries = new Map<string, DlEntry>();
let snapshot: DlEntry[] = [];
let version = 0;
const listeners = new Set<() => void>();
// pending queue of {repoId, filename} waiting for a concurrency slot
const queue: { repoId: string; filename: string }[] = [];
const doneCbs = new Set<(path: string) => void>();

function bump() {
  version++;
  snapshot = [...entries.values()].sort((a, b) => b.startedAt - a.startedAt);
  listeners.forEach((l) => l());
}

export function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getSnapshot(): DlEntry[] {
  return snapshot;
}

export function getVersion(): number {
  return version;
}

function entryFor(p: EvPayload): DlEntry | undefined {
  return entries.get(p.key);
}

/** Fire all done callbacks (used to rescan the index after files land). */
function fireDone(path: string) {
  doneCbs.forEach((cb) => cb(path));
}

function launch(repoId: string, filename: string) {
  api
    .downloadStart(repoId, filename)
    .catch((e) => {
      const key = `${repoId}/${filename}`;
      if (!String(e).includes("already downloading")) {
        entries.set(key, {
          key,
          name: key,
          total: 0,
          done: 0,
          speed: 0,
          startedAt: Date.now(),
          status: "error",
          err: String(e),
        });
        bump();
        setTimeout(() => {
          entries.delete(key);
          bump();
        }, 8000);
      }
    });
  // optimistic row so the drawer reacts instantly (backend also emits dl-start)
  const key = `${repoId}/${filename}`;
  if (!entries.has(key)) {
    entries.set(key, {
      key,
      name: key,
      total: 0,
      done: 0,
      speed: 0,
      startedAt: Date.now(),
      status: "active",
    });
    bump();
  }
}

function pump() {
  const active = [...entries.values()].filter((e) => e.status === "active").length;
  let slots = MAX_ACTIVE - active;
  while (slots > 0 && queue.length > 0) {
    const next = queue.shift()!;
    launch(next.repoId, next.filename);
    slots--;
  }
}

async function waitForTerminal(key: string): Promise<void> {
  await new Promise<void>((resolve) => {
    const unsub = subscribe(() => {
      const e = entries.get(key);
      if (!e || e.status !== "active") {
        unsub();
        resolve();
      }
    });
    // already terminal?
    const e = entries.get(key);
    if (!e || e.status !== "active") {
      unsub();
      resolve();
    }
  });
}

/**
 * Queue one file for download. Safe to call repeatedly — duplicates are
 * coalesced by backend and by the optimistic guard.
 */
export function enqueue(repoId: string, filename: string) {
  const key = `${repoId}/${filename}`;
  if (entries.has(key) && entries.get(key)!.status === "active") return;
  if (queue.some((q) => q.filename === filename && q.repoId === repoId)) return;
  queue.push({ repoId, filename });
  pump();
}

/**
 * Queue an ordered list of files (e.g. a shard set), starting each only once
 * the previous finishes so multi-part GGUFs land contiguously.
 */
export async function enqueueSequence(repoId: string, filenames: string[]) {
  enqueue(repoId, filenames[0]);
  for (let i = 1; i < filenames.length; i++) {
    await waitForTerminal(`${repoId}/${filenames[i - 1]}`);
    enqueue(repoId, filenames[i]);
  }
}

export function cancel(key: string) {
  api.downloadCancel(key).catch(() => {});
}

/** Manually dismiss a finished/failed row (errors no longer self-delete). */
export function removeEntry(key: string) {
  entries.delete(key);
  bump();
}

/** Register a callback fired whenever any file lands on disk. */
export function onDone(cb: (path: string) => void): () => void {
  doneCbs.add(cb);
  return () => doneCbs.delete(cb);
}

/** Ordered shard-set members containing `chosen` (single file if not a shard). */
export function shardSet(chosen: string, allNames: string[]): string[] {
  const splitShard = (name: string): [string, number, number] | null => {
    const dot = name.lastIndexOf(".");
    if (dot < 0) return null;
    const ext = name.slice(dot + 1).toLowerCase();
    if (ext !== "gguf" && ext !== "safetensors") return null;
    const base = name.slice(0, dot);
    const five = (s: string) => s.length === 5 && /^\d{5}$/.test(s);
    const dashTotal = base.lastIndexOf("-");
    if (dashTotal < 0) return null;
    const totalStr = base.slice(dashTotal + 1);
    if (!five(totalStr)) return null;
    const beforeOf = base.slice(0, dashTotal);
    if (!beforeOf.endsWith("-of")) return null;
    const rest = beforeOf.slice(0, -3);
    const dashPart = rest.lastIndexOf("-");
    if (dashPart < 0) return null;
    const partStr = rest.slice(dashPart + 1);
    if (!five(partStr)) return null;
    const prefix = rest.slice(0, dashPart);
    if (!prefix) return null;
    return [prefix, parseInt(partStr, 10), parseInt(totalStr, 10)];
  };
  const m = splitShard(chosen);
  if (!m) return [chosen];
  const [prefix, , declared] = m;
  const parts = allNames
    .map(splitShard)
    .map((x, i): [number, string] | null => (x && x[0] === prefix && x[2] === declared ? [x[1], allNames[i]] : null))
    .filter((x): x is [number, string] => x !== null)
    .sort((a, b) => a[0] - b[0]);
  return parts.length === declared ? parts.map(([, n]) => n) : [chosen];
}

let initialized = false;

/** Idempotently attach the Tauri event listeners backing this store. */
export function init(): void {
  if (initialized || typeof window === "undefined") return;
  initialized = true;

  listen<EvPayload>("dl-start", ({ payload }) => {
    if (!entries.has(payload.key)) {
      entries.set(payload.key, {
        key: payload.key,
        name: payload.key,
        total: 0,
        done: 0,
        speed: 0,
        startedAt: Date.now(),
        status: "active",
      });
    }
    bump();
    pump();
  });

  listen<EvPayload>("dl-progress", ({ payload }) => {
    const e = entryFor(payload);
    if (!e) return;
    const now = performance.now();
    if (payload.total != null && payload.total > 0) e.total = payload.total;
    if (payload.done != null) {
      const prevDone = e.done;
      if (payload.done > prevDone && e.lastTickMs != null) {
        const secs = (now - e.lastTickMs) / 1000;
        if (secs > 0.05) {
          const inst = ((payload.done - prevDone) as number) / secs;
          e.speed = e.speed === 0 ? inst : e.speed * 0.6 + inst * 0.4;
        }
      }
      e.lastTickMs = now;
      e.done = payload.done;
    }
    bump();
  });

  listen<EvPayload>("dl-done", ({ payload }) => {
    const e = entryFor(payload);
    if (e) {
      e.status = "done";
      e.done = e.total > 0 ? e.total : e.done;
      setTimeout(() => {
        entries.delete(payload.key);
        bump();
      }, 9000);
    } else {
      // fast completion raced past progress events
      entries.set(payload.key, {
        key: payload.key,
        name: payload.rfilename,
        total: 0,
        done: 0,
        speed: 0,
        startedAt: Date.now(),
        status: "done",
      });
      bump();
    }
    fireDone(payload.path ?? "");
    setTimeout(() => {
      bump();
      pump();
    }, 60);
  });

  listen<EvPayload>("dl-error", ({ payload }) => {
    const cancelled = payload.error === "cancelled";
    if (cancelled) {
      entries.delete(payload.key);
    } else {
      const e = entryFor(payload);
      if (e) {
        e.status = "error";
        e.err = payload.error;
        // keep the row on screen until the user dismisses it — silent failures
        // are how "downloads don't work" bugs survive
        bump();
        return;
      }
    }
    bump();
    pump();
  });
}
