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
    if (!hostRef.current) {
      console.error(`[TuiPane ${pane.id}] hostRef.current is null — cannot init xterm`);
      return;
    }
    const hostEl = hostRef.current;
    const rect = hostEl.getBoundingClientRect();
    console.log(`[TuiPane ${pane.id}] mounting — host size: ${rect.width}×${rect.height}`);

    const term = new Terminal({
      cursorBlink: true,
      fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      fontSize: 12,
      theme: { background: "#0d1117", foreground: "#e6edf3" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostEl);
    fit.fit();
    console.log(`[TuiPane ${pane.id}] xterm opened — cols=${term.cols} rows=${term.rows} host=${hostEl.clientWidth}×${hostEl.clientHeight}`);
    if (term.cols === 0 || term.rows === 0) {
      console.error(`[TuiPane ${pane.id}] FitAddon returned 0 dimensions! host clientSize=${hostEl.clientWidth}×${hostEl.clientHeight} offsetSize=${hostEl.offsetWidth}×${hostEl.offsetHeight}`);
    }
    termRef.current = term;
    fitRef.current = fit;

    let dataCount = 0;
    let byteCount = 0;

    // raw PTY bytes for this pane → terminal
    let unData: (() => void) | undefined;
    let unExited: (() => void) | undefined;
    listen<{ id: string; bytes: number[] }>("tui-data", (e) => {
      if (e.payload.id !== pane.id) return;
      dataCount++;
      byteCount += e.payload.bytes.length;
      if (dataCount <= 3) {
        console.log(`[TuiPane ${pane.id}] tui-data #${dataCount}: ${e.payload.bytes.length} bytes (total ${byteCount})`);
      }
      term.write(new Uint8Array(e.payload.bytes));
    }).then((f) => {
      unData = f;
      console.log(`[TuiPane ${pane.id}] tui-data listener registered`);
    });
    listen<{ id: string; code: number }>("tui-exited", (e) => {
      if (e.payload.id !== pane.id) return;
      console.log(`[TuiPane ${pane.id}] tui-exited code=${e.payload.code} (received ${dataCount} data events, ${byteCount} bytes total)`);
      onExitedRef.current(pane.id);
    }).then((f) => (unExited = f));

    // keystrokes → PTY master
    const d = term.onData((chunk) => {
      void api.tuiWrite(pane.id, Array.from(chunk, (c) => c.charCodeAt(0)));
    });

    return () => {
      console.log(`[TuiPane ${pane.id}] unmounting — received ${dataCount} data events, ${byteCount} bytes total`);
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
    let resizeCount = 0;
    const ro = new ResizeObserver((entries) => {
      resizeCount++;
      const entry = entries[0];
      const w = entry?.contentRect.width ?? 0;
      const h = entry?.contentRect.height ?? 0;
      fit.fit();
      if (resizeCount <= 3 || term.cols === 0 || term.rows === 0) {
        console.log(`[TuiPane ${pane.id}] resize #${resizeCount}: host=${w.toFixed(0)}×${h.toFixed(0)} → cols=${term.cols} rows=${term.rows}`);
      }
      void api.tuiResize(pane.id, term.cols, term.rows);
    });
    if (hostRef.current) ro.observe(hostRef.current);
    return () => ro.disconnect();
  }, [pane.id]);

  return (
    <div
      ref={hostRef}
      style={{ width: "100%", height: "100%", background: "#0d1117" }}
    />
  );
}
