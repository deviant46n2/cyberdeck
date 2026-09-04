// Agent session store: persists session state across app restarts via the
// backend SQLite store, streams live session events, and provides handoff
// generation. Replaces the in-memory-only sessions array in Workspace.tsx.
//
// Lifecycle:
//   createSession() → opencodeRun(sessionId) → [events stream] → complete/error/stop
//   handoff can be generated at any point for a completed/stopped session.
//   Sessions survive app restarts — listSessions() returns all persisted sessions.

import { listen } from "@tauri-apps/api/event";
import * as api from "../api";

// --- Types ---

export type SessionStatus =
  | "pending"
  | "running"
  | "complete"
  | "stopped"
  | "error"
  | "disconnected";

export interface Session {
  id: string;
  projectDir: string;
  agent: string;
  model: string;
  task: string;
  status: SessionStatus;
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
  autoMode: boolean;
  ctxSize: number;
  exitCode: number | null;
  errorMessage: string | null;
  hasHandoff: boolean;
  /** Live output lines (not persisted — rebuilt on reconnect or from events). */
  log: string[];
}

// --- State ---

let sessions: Session[] = [];
let snapshot: Session[] = [];
let version = 0;
const listeners = new Set<() => void>();

function bump() {
  version++;
  snapshot = [...sessions];
  listeners.forEach((l) => l());
}

export function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getSnapshot(): Session[] {
  return snapshot;
}

export function getVersion(): number {
  return version;
}

/** Find a session by id from the live state. */
export function find(id: string): Session | undefined {
  return sessions.find((s) => s.id === id);
}

// --- Conversions ---

function fromView(v: api.SessionView): Session {
  return {
    id: v.id,
    projectDir: v.project_dir,
    agent: v.agent,
    model: v.model,
    task: v.task,
    status: v.status as SessionStatus,
    createdAt: v.created_at,
    startedAt: v.started_at,
    completedAt: v.completed_at,
    autoMode: v.auto_mode,
    ctxSize: v.ctx_size,
    exitCode: v.exit_code,
    errorMessage: v.error_message,
    hasHandoff: v.has_handoff,
    log: [],
  };
}

// --- Actions ---

/** Load all sessions from the backend on boot / reconnect. */
export async function loadSessions(status?: string | null): Promise<void> {
  try {
    const views = await api.listSessions(status ?? null);
    // Merge: keep any live log lines we already have, update metadata from DB.
    const liveMap = new Map(sessions.map((s) => [s.id, s.log]));
    sessions = views.map((v) => ({
      ...fromView(v),
      log: liveMap.get(v.id) ?? [],
    }));
    bump();
  } catch (e) {
    console.error("[sessions] loadSessions failed:", e);
  }
}

/** Create a new session in the DB and return its id. */
export async function createSession(opts: {
  projectDir: string;
  agent: string;
  model: string;
  task: string;
  autoMode: boolean;
  ctxSize: number;
}): Promise<string> {
  const id = await api.createSession(opts);
  // Optimistically add to local state.
  sessions.unshift({
    id,
    projectDir: opts.projectDir,
    agent: opts.agent,
    model: opts.model,
    task: opts.task,
    status: "pending",
    createdAt: Math.floor(Date.now() / 1000),
    startedAt: null,
    completedAt: null,
    autoMode: opts.autoMode,
    ctxSize: opts.ctxSize,
    exitCode: null,
    errorMessage: null,
    hasHandoff: false,
    log: [],
  });
  bump();
  return id;
}

/** Generate a handoff document for a session. */
export async function getHandoff(sessionId: string): Promise<string> {
  return api.generateHandoff(sessionId);
}

/** Delete a session from the DB. */
export async function deleteSession(id: string): Promise<void> {
  await api.deleteSession(id);
  sessions = sessions.filter((s) => s.id !== id);
  bump();
}

/** Get events for a session (log replay). */
export async function loadEvents(sessionId: string): Promise<api.SessionEvent[]> {
  return api.getSessionEvents(sessionId);
}

// --- Event listeners ---

let initialized = false;

/** Idempotently attach the Tauri event listeners backing this store. */
export function init(): void {
  if (initialized || typeof window === "undefined") return;
  initialized = true;

  // Session status changes (running, complete, error, stopped).
  listen<{ session_id: string; status: string; exit_code: number | null; error: string | null }>(
    "session-status",
    ({ payload }) => {
      const s = sessions.find((x) => x.id === payload.session_id);
      if (s) {
        s.status = payload.status as SessionStatus;
        if (payload.exit_code != null) s.exitCode = payload.exit_code;
        if (payload.error) s.errorMessage = payload.error;
        if (payload.status === "complete" || payload.status === "stopped" || payload.status === "error") {
          s.completedAt = Math.floor(Date.now() / 1000);
          s.hasHandoff = true;
        }
        bump();
      }
    }
  );

  // Session output events (line-by-line streaming).
  listen<{ session_id: string; kind: string; stream: string; text: string }>(
    "session-event",
    ({ payload }) => {
      if (payload.kind === "line") {
        const s = sessions.find((x) => x.id === payload.session_id);
        if (s) {
          s.log.push(payload.text);
          bump();
        }
      }
    }
  );
}

/** Clear the log of a session. */
export function clearLog(sessionId: string): void {
  const s = sessions.find((x) => x.id === sessionId);
  if (s) {
    s.log = [];
    bump();
  }
}
