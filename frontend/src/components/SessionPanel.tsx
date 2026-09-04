// SessionPanel: live session status, controls, output log, and handoff.
// Replaces the ad-hoc session cards in Workspace.tsx with a structured panel
// that communicates session status, provides obvious controls, and generates
// handoff documents for completed/stopped sessions.

import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../api";
import * as sessions from "../lib/sessions";

const STATUS_COLORS: Record<string, string> = {
  pending: "var(--dim2)",
  running: "var(--cyan)",
  complete: "var(--pass)",
  stopped: "var(--warn)",
  error: "var(--oom)",
  disconnected: "var(--dim2)",
};

const STATUS_LABELS: Record<string, string> = {
  pending: "PENDING",
  running: "WORKING",
  complete: "COMPLETE",
  stopped: "STOPPED",
  error: "ERROR",
  disconnected: "OFFLINE",
};

export default function SessionPanel({
  sessionId,
  onDismiss,
  onContinue,
}: {
  sessionId: string;
  onDismiss: () => void;
  onContinue?: (sessionId: string) => void;
}) {
  const [session, setSession] = useState<sessions.Session | null>(null);
  const [handoff, setHandoff] = useState<string | null>(null);
  const [handoffLoading, setHandoffLoading] = useState(false);
  const [copied, setCopied] = useState(false);
  const logRef = useRef<HTMLDivElement>(null);
  const unsubRef = useRef<(() => void) | null>(null);

  // Subscribe to store changes.
  useEffect(() => {
    const unsub = sessions.subscribe(() => {
      const s = sessions.find(sessionId);
      setSession(s ?? null);
    });
    unsubRef.current = unsub;
    // Initial read.
    const s = sessions.find(sessionId);
    setSession(s ?? null);
    return () => unsub();
  }, [sessionId]);

  // Auto-scroll log on new output.
  useEffect(() => {
    if (logRef.current) {
      const el = logRef.current;
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
      if (atBottom) {
        el.scrollTop = el.scrollHeight;
      }
    }
  }, [session?.log.length]);

  // Generate handoff when session completes/stops.
  const refreshHandoff = useCallback(async () => {
    if (!session) return;
    if (!session.hasHandoff && session.status !== "complete" && session.status !== "stopped") {
      setHandoff(null);
      return;
    }
    setHandoffLoading(true);
    try {
      const h = await sessions.getHandoff(sessionId);
      setHandoff(h);
    } catch (e) {
      setHandoff(`[handoff error: ${String(e)}]`);
    } finally {
      setHandoffLoading(false);
    }
  }, [session, sessionId]);

  useEffect(() => {
    if (session?.hasHandoff || session?.status === "complete" || session?.status === "stopped") {
      void refreshHandoff();
    }
  }, [session?.status, session?.hasHandoff, refreshHandoff]);

  const copyHandoff = useCallback(async () => {
    if (!handoff) return;
    try {
      await navigator.clipboard.writeText(handoff);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback: select text.
    }
  }, [handoff]);

  if (!session) {
    return (
      <div style={{ padding: 12, color: "var(--dim2)", fontSize: 11 }}>
        Session not found or loading…
      </div>
    );
  }

  const statusColor = STATUS_COLORS[session.status] ?? "var(--dim2)";
  const statusLabel = STATUS_LABELS[session.status] ?? session.status.toUpperCase();
  const isTerminal = ["complete", "stopped", "error"].includes(session.status);
  const isRunning = session.status === "running";

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        background: "var(--panel)",
        borderRadius: 6,
        border: "1px solid var(--line)",
        overflow: "hidden",
        fontSize: 12,
      }}
    >
      {/* Header */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "8px 12px",
          borderBottom: "1px solid var(--line)",
          flexShrink: 0,
        }}
      >
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: "50%",
            background: statusColor,
            flexShrink: 0,
          }}
        />
        <span style={{ fontWeight: 700, fontSize: 11, letterSpacing: 0.5, color: statusColor }}>
          {statusLabel}
        </span>
        <span style={{ color: "var(--dim2)", fontSize: 10, marginLeft: 4 }}>
          {session.model || "no model"}
        </span>
        <span style={{ color: "var(--dim2)", fontSize: 10 }}>·</span>
        <span style={{ color: "var(--dim2)", fontSize: 10, flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {session.task || "no task"}
        </span>
        {isRunning && (
          <button
            className="ghost"
            style={{ fontSize: 10, color: "var(--oom)", borderColor: "var(--oom)", padding: "2px 8px" }}
            onClick={() => void api.opencodeStop(session.id)}
            title="Stop this session"
          >
            ■ STOP
          </button>
        )}
        {isTerminal && onContinue && (
          <button
            className="ghost"
            style={{ fontSize: 10, color: "var(--cyan)", borderColor: "var(--cyan)", padding: "2px 8px" }}
            onClick={() => onContinue(session.id)}
            title="Continue this session with a new prompt"
          >
            ▶ CONTINUE
          </button>
        )}
        <button
          className="ghost"
          style={{ fontSize: 10, padding: "2px 6px" }}
          onClick={onDismiss}
          title="Close this panel"
        >
          ✕
        </button>
      </div>

      {/* Meta row */}
      <div
        style={{
          display: "flex",
          gap: 12,
          padding: "6px 12px",
          fontSize: 10,
          color: "var(--dim2)",
          borderBottom: "1px solid var(--line)",
          flexShrink: 0,
          flexWrap: "wrap",
        }}
      >
        <span>📁 {session.projectDir}</span>
        {session.agent && <span>🤖 {session.agent}</span>}
        {session.startedAt && (
          <span>
            ⏱ {formatDuration(session.startedAt, session.completedAt)}
          </span>
        )}
        {session.exitCode != null && (
          <span style={{ color: session.exitCode === 0 ? "var(--pass)" : "var(--oom)" }}>
            exit {session.exitCode}
          </span>
        )}
        {session.errorMessage && (
          <span style={{ color: "var(--oom)" }}>⚠ {session.errorMessage}</span>
        )}
      </div>

      {/* Log output */}
      <div
        ref={logRef}
        style={{
          flex: 1,
          minHeight: 0,
          overflow: "auto",
          padding: "8px 12px",
          fontFamily: '"JetBrains Mono", "Fira Code", monospace',
          fontSize: 11,
          lineHeight: 1.5,
          background: "#0d1117",
          color: "#e6edf3",
        }}
      >
        {session.log.length === 0 && (
          <div style={{ color: "var(--dim2)", fontStyle: "italic" }}>
            {isRunning ? "Waiting for output…" : "No output captured."}
          </div>
        )}
        {session.log.map((line, i) => (
          <div key={i} style={{ whiteSpace: "pre-wrap", wordBreak: "break-all" }}>
            {line}
          </div>
        ))}
      </div>

      {/* Handoff section */}
      {(isTerminal || session.hasHandoff) && (
        <div
          style={{
            borderTop: "1px solid var(--line)",
            padding: "8px 12px",
            flexShrink: 0,
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              marginBottom: 6,
            }}
          >
            <span style={{ fontSize: 10, fontWeight: 700, color: "var(--muted)", letterSpacing: 0.5 }}>
              HANDOFF
            </span>
            {handoffLoading && (
              <span style={{ fontSize: 10, color: "var(--dim2)" }}>generating…</span>
            )}
            {handoff && (
              <button
                className="ghost"
                style={{
                  fontSize: 10,
                  padding: "2px 8px",
                  color: copied ? "var(--pass)" : "var(--cyan)",
                  borderColor: copied ? "var(--pass)" : "var(--cyan)",
                }}
                onClick={() => void copyHandoff()}
              >
                {copied ? "✓ COPIED" : "📋 COPY"}
              </button>
            )}
          </div>
          {handoff && (
            <pre
              style={{
                maxHeight: 160,
                overflow: "auto",
                fontSize: 10,
                lineHeight: 1.4,
                color: "var(--text)",
                background: "var(--panel-2)",
                padding: 8,
                borderRadius: 4,
                border: "1px solid var(--line)",
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
                margin: 0,
              }}
            >
              {handoff}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

function formatDuration(startedAt: number, completedAt: number | null): string {
  const end = completedAt ?? Math.floor(Date.now() / 1000);
  const dur = end - startedAt;
  if (dur < 60) return `${dur}s`;
  const m = Math.floor(dur / 60);
  const s = dur % 60;
  return `${m}m ${s}s`;
}
