//! A single embedded opencode TUI as an xterm.js pane on the HUD canvas.
//!
//! Renders the raw PTY byte stream (`tui-data` events for this pane id) into an
//! xterm terminal, forwards keystrokes back to the pane's PTY master via
//! `tui_write`, and reports resize via `tui_resize`. Draggable/resizable by the
//! surrounding canvas wrapper — this component owns only the live terminal.

import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import * as api from "../api";
import "@xterm/xterm/css/xterm.css";

export default function TuiPane({
  pane,
  onExited,
}: {
  pane: { id: string; dir: string };
  onExited: (id: string) => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const onExitedRef = useRef(onExited);
  onExitedRef.current = onExited;

  useEffect(() => {
    if (!hostRef.current) return;
    const term = new Terminal({
      cursorBlink: true,
      fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      fontSize: 12,
      theme: { background: "#060610", foreground: "#d6d6e0" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    fit.fit();
    termRef.current = term;
    fitRef.current = fit;

    // raw PTY bytes for this pane → terminal
    let unData: (() => void) | undefined;
    let unExited: (() => void) | undefined;
    listen<{ id: string; bytes: number[] }>("tui-data", (e) => {
      if (e.payload.id !== pane.id) return;
      term.write(new Uint8Array(e.payload.bytes));
    }).then((f) => (unData = f));
    listen<{ id: string; code: number }>("tui-exited", (e) => {
      if (e.payload.id !== pane.id) return;
      onExitedRef.current(pane.id);
    }).then((f) => (unExited = f));

    // keystrokes → PTY master
    const d = term.onData((chunk) => {
      void api.tuiWrite(pane.id, Array.from(chunk, (c) => c.charCodeAt(0)));
    });

    return () => {
      d.dispose();
      unData?.();
      unExited?.();
      term.dispose();
    };
  }, [pane.id]);

  // resize → tell the PTY; also refit xterm to its host size
  useEffect(() => {
    const fit = fitRef.current;
    const term = termRef.current;
    if (!fit || !term) return;
    const ro = new ResizeObserver(() => {
      fit.fit();
      void api.tuiResize(pane.id, term.cols, term.rows);
    });
    if (hostRef.current) ro.observe(hostRef.current);
    return () => ro.disconnect();
  }, [pane.id]);

  return (
    <div
      ref={hostRef}
      style={{ width: "100%", height: "100%", background: "#060610" }}
    />
  );
}
