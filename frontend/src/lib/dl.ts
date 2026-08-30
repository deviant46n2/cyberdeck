// Global background-download store, Steam-style. Backed by the dl-* Tauri
// events emitted by deck-tauri's job registry and consumed by the DOWNLOADS
// manager view (and queues kicked off from MARKET).
//
// Lifecycle: enqueue() appends to `order` (the priority queue, front = next
// to start). pump() fills up to MAX_ACTIVE slots from the frontmost queued
// entries. STOP on an active/queued item parks it as `paused` (backend keeps
// the .part — START resumes it). Done/error rows persist until dismissed.
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

export type DlStatus = "queued" | "active" | "paused" | "done" | "error";

export interface DlEntry {
  repoId: string;
  rfilename: string;
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
  /** resolved on-disk path once the file has landed (dl-done payload). */
  path?: string;
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

/** Bookkeeping for a multi-part shard set: index it only once every declared
 * member has landed, so a half-downloaded model never enters the vault. */
interface SetBook {
  repoId: string;
  files: string[];
  paths: (string | null)[];
  done: boolean[];
}
const sets = new Map<string, SetBook>();

/** Priority-ordered keys of non-terminal entries (active+queued+paused).
 * Front = downloads the runner tries first. */
const order: string[] = [];

const entries = new Map<string, DlEntry>();
let snapshot: DlEntry[] = [];
let version = 0;
const listeners = new Set<() => void>();
const doneCbs = new Set<(path: string) => void>();

function bump() {
  version++;
  snapshot = buildSnapshot();
  listeners.forEach((l) => l());
}

function buildSnapshot(): DlEntry[] {
  const mid: DlEntry[] = [];
  for (const k of order) {
    const e = entries.get(k);
    if (e) mid.push(e);
  }
  const term = [...entries.values()]
    .filter((e) => e.status === "done" || e.status === "error")
    .sort((a, b) => b.startedAt - a.startedAt);
  return [...mid, ...term];
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

/** Count of actually-running transfers (for a sidebar badge). */
export function activeCount(): number {
  let n = 0;
  for (const e of entries.values()) if (e.status === "active") n++;
  return n;
}

function entryFor(key: string): DlEntry | undefined {
  return entries.get(key);
}

/** Fire all done callbacks (used to rescan the index after files land). */
function fireDone(path: string) {
  doneCbs.forEach((cb) => cb(path));
}

/** Terminal lifecycle for a landed key: mark done, drop from the run queue,
 * index (set-aware), fire the rescan callback. Shared by the `dl-done` event
 * handler and the launch-time reconcile so both paths converge identically. */
function finalizeDone(key: string, landed: string, repoId?: string, rfilename?: string) {
  const e = entryFor(key);
  if (e) {
    e.status = "done";
    e.done = e.total > 0 ? e.total : e.done;
    if (landed) e.path = landed;
  } else {
    entries.set(key, {
      repoId: repoId ?? "",
      rfilename: rfilename ?? "",
      key,
      name: key,
      total: 0,
      done: 0,
      speed: 0,
      startedAt: Date.now(),
      status: "done",
      path: landed || undefined,
    });
    addToQueue(key);
  }
  const i = order.indexOf(key);
  if (i >= 0) order.splice(i, 1);
  void indexOnLanded(key, landed);
  fireDone(landed);
  bump();
  pump();
}

/** Converge a key's store row to the backend's authoritative state. Events
 * (`dl-start`/`dl-done`/…) remain the live channel; this is the safety net
 * so a single dropped event can never leave a completed transfer pinned in
 * `queued` while its `dl-done` never arrives. */
function reconcile(key: string, st: api.DownloadState) {
  const e = entryFor(key);
  if (!e) return;
  switch (st.status) {
    case "done":
      if (e.status !== "done") finalizeDone(key, st.path ?? "");
      break;
    case "paused":
      e.status = "paused";
      bump();
      break;
    case "error":
      if (e.status !== "error") {
        e.status = "error";
        e.err = st.error ?? "failed";
        bump();
        pump();
      }
      break;
    case "active":
      if (e.status !== "active") {
        e.status = "active";
        bump();
        pump();
      }
      break;
    default:
      // queued: the store row already matches — events confirm the launch.
      break;
  }
}

function launch(key: string) {
  const e = entries.get(key);
  if (!e) return;
  api
    .downloadStart(e.repoId, e.rfilename)
    .then(async () => {
      // Convergence net: read the backend's authoritative state right after
      // start resolves so a dropped start/done event converges immediately.
      // Best-effort — events remain the primary channel.
      try {
        const states = await api.downloadStates([key]);
        const st = states.find((s) => s.key === key);
        if (st) reconcile(key, st);
      } catch {
        /* reconcile is best-effort */
      }
    })
    .catch((err) => {
      const msg = String(err);
      const cur = entries.get(key);
      if (!cur) return;
      if (msg.includes("already downloading")) {
        // Backend already streaming this key; the dl-start event will confirm.
        cur.status = "active";
        bump();
        pump();
      } else {
        cur.status = "error";
        cur.err = msg;
        bump();
      }
    });
}

function pump() {
  let active = 0;
  for (const e of entries.values()) if (e.status === "active") active++;
  let slots = MAX_ACTIVE - active;
  for (const k of order) {
    if (slots <= 0) break;
    const e = entries.get(k);
    if (!e || e.status !== "queued") continue;
    launch(k);
    slots--;
  }
}

async function waitForTerminal(key: string): Promise<void> {
  await new Promise<void>((resolve) => {
    const unsub = subscribe(() => {
      const e = entries.get(key);
      if (!e || (e.status !== "active" && e.status !== "queued")) {
        unsub();
        resolve();
      }
    });
    const e = entries.get(key);
    if (!e || (e.status !== "active" && e.status !== "queued")) {
      unsub();
      resolve();
    }
  });
}

function setKey(repoId: string, filenames: string[]): string {
  return `${repoId}\u0000${filenames.join("\u0000")}`;
}

/**
 * Index everything that just landed: the whole shard set once all parts are
 * done, or the single file otherwise. Deterministic — the model appears in the
 * DB the moment the last byte is on disk, independent of the debounced refresh
 * that also runs for consistency. Non-fatal: a failed index still gets picked
 * up by the next full scan.
 */
async function indexOnLanded(key: string, landed: string): Promise<void> {
  let target: string[] | null = null;
  let matchedSet = false;
  for (const bk of sets.values()) {
    const idx = bk.files.findIndex((f) => `${bk.repoId}/${f}` === key);
    if (idx < 0) continue;
    matchedSet = true;
    bk.done[idx] = true;
    if (landed) bk.paths[idx] = landed;
    if (bk.done.every(Boolean)) {
      const paths = bk.paths.filter((p): p is string => p != null);
      if (paths.length === bk.files.length) target = paths;
      sets.delete(setKey(bk.repoId, bk.files));
    }
    break;
  }
  // A single file (no shard book) indexes immediately. A partial shard set is
  // NOT indexed — it waits for every member to land, keeping the vault
  // half-model-free.
  if (!target && landed && !matchedSet) target = [landed];
  if (!target) return;
  try {
    await api.indexDownloaded(target);
  } catch {
    // poured off the debounced rescan in App.tsx
  }
}

function addToQueue(key: string) {
  if (!order.includes(key)) order.push(key);
}

/**
 * Queue one file for download. Safe to call repeatedly — an entry that is
 * queued/active/paused/done is left alone; an errored entry restarts.
 */
export function enqueue(repoId: string, filename: string) {
  const key = `${repoId}/${filename}`;
  const existing = entries.get(key);
  if (existing && existing.status !== "error") return;
  if (existing) {
    existing.status = "queued";
    existing.err = undefined;
    existing.total = existing.done = existing.speed = 0;
    existing.startedAt = Date.now();
    addToQueue(key);
    bump();
    pump();
    return;
  }
  entries.set(key, {
    repoId,
    rfilename: filename,
    key,
    name: key,
    total: 0,
    done: 0,
    speed: 0,
    startedAt: Date.now(),
    status: "queued",
  });
  addToQueue(key);
  bump();
  pump();
}

/**
 * Queue an ordered list of files (e.g. a shard set), starting each only once
 * the previous finishes so multi-part GGUFs land contiguously. Multi-file sets
 * are tracked so the vault index sees them only after every part lands.
 */
export async function enqueueSequence(repoId: string, filenames: string[]) {
  if (filenames.length > 1) {
    sets.set(setKey(repoId, filenames), {
      repoId,
      files: filenames,
      paths: filenames.map(() => null),
      done: filenames.map(() => false),
    });
  }
  enqueue(repoId, filenames[0]);
  for (let i = 1; i < filenames.length; i++) {
    await waitForTerminal(`${repoId}/${filenames[i - 1]}`);
    enqueue(repoId, filenames[i]);
  }
}

/** Alias kept for older callers ({@link stop}). */
export const cancel = stop;

/**
 * STOP: pause an active transfer (backend keeps the `.part` so START can
 * resume) or hold a queued one before it ever launches. The entry keeps its
 * position in the priority queue.
 */
export function stop(key: string) {
  const e = entries.get(key);
  if (!e || (e.status !== "active" && e.status !== "queued")) return;
  e.status = "paused";
  bump();
  void api.downloadCancel(key).catch(() => {});
  pump();
}

/** START / RETRY: unpark a paused or errored entry back into the queue. */
export function start(key: string) {
  const e = entries.get(key);
  if (!e || (e.status !== "paused" && e.status !== "error")) return;
  e.status = "queued";
  addToQueue(key);
  bump();
  pump();
}

/** Move an entry up/down in the priority queue (front = runs next). */
export function movePriority(key: string, dir: 1 | -1) {
  const i = order.indexOf(key);
  const j = i + dir;
  if (i < 0 || j < 0 || j >= order.length) return;
  [order[i], order[j]] = [order[j], order[i]];
  bump();
}

/** REMOVE: cancel if running, drop the `.part` if any, and forget the row.
 * Completed files on disk are left alone (that's the VAULT's job). */
export async function discard(key: string) {
  const e = entries.get(key);
  if (!e) return;
  const wasActive = e.status === "active";
  const hadPart = e.status === "paused" || e.status === "error";
  if (wasActive) {
    try {
      await api.downloadCancel(key);
    } catch {
      /* already gone */
    }
  }
  if (wasActive || hadPart) {
    try {
      await api.downloadRemove(key, e.rfilename);
    } catch {
      /* best-effort */
    }
  }
  entries.delete(key);
  const i = order.indexOf(key);
  if (i >= 0) order.splice(i, 1);
  bump();
  pump();
}

/** Dismiss every finished/failed row (files already on disk, or gone). */
export function clearFinished() {
  for (const [k, e] of entries) {
    if (e.status === "done" || e.status === "error") entries.delete(k);
  }
  bump();
  pump();
}

/** Manually dismiss a finished/failed row (legacy alias of clearFinished row). */
export function removeEntry(key: string) {
  entries.delete(key);
  const i = order.indexOf(key);
  if (i >= 0) order.splice(i, 1);
  bump();
  pump();
}

/** Register a callback fired whenever any file lands on disk. */
export function onDone(cb: (path: string) => void): () => void {
  doneCbs.add(cb);
  return () => doneCbs.delete(cb);
}

let initialized = false;

/** Idempotently attach the Tauri event listeners backing this store. */
export function init(): void {
  if (initialized || typeof window === "undefined") return;
  initialized = true;

  listen<EvPayload>("dl-start", ({ payload }) => {
    let e = entryFor(payload.key);
    if (!e) {
      entries.set(payload.key, {
        repoId: payload.repo_id,
        rfilename: payload.rfilename,
        key: payload.key,
        name: payload.key,
        total: 0,
        done: 0,
        speed: 0,
        startedAt: Date.now(),
        status: "active",
      });
      addToQueue(payload.key);
    } else {
      e.status = "active";
    }
    bump();
    pump();
  });

  listen<EvPayload>("dl-progress", ({ payload }) => {
    const e = entryFor(payload.key);
    if (!e) return;
    const now = performance.now();
    if (payload.total != null && payload.total > 0) e.total = payload.total;
    if (payload.done != null) {
      const prevDone = e.done;
      if (payload.done > prevDone && e.lastTickMs != null && e.status === "active") {
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
    finalizeDone(payload.key, payload.path ?? "", payload.repo_id, payload.rfilename);
  });

  listen<EvPayload>("dl-error", ({ payload }) => {
    const e = entryFor(payload.key);
    if (payload.error === "cancelled") {
      // User STOP — the `.part` is kept; the row sits paused awaiting START.
      if (e) e.status = "paused";
    } else {
      if (e) {
        e.status = "error";
        e.err = payload.error;
      } else {
        entries.set(payload.key, {
          repoId: payload.repo_id,
          rfilename: payload.rfilename,
          key: payload.key,
          name: payload.key,
          total: 0,
          done: 0,
          speed: 0,
          startedAt: Date.now(),
          status: "error",
          err: payload.error,
        });
        addToQueue(payload.key);
      }
    }
    bump();
    pump();
  });
}