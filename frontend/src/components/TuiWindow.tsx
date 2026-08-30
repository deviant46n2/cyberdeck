//! Draggable + resizable window frame that hosts an embedded opencode TUI
//! (`TuiPane`). Position/size live in a ref map owned by the canvas host so
//! dragging never churns React state; a single repaint updates the visual.

import { useRef, useState, type ReactNode } from "react";
import * as api from "../api";
import TuiPane from "./TuiPane";

export default function TuiWindow({
  pane,
  pos,
  onPos,
  onDismiss,
}: {
  pane: { id: string; dir: string };
  pos: { x: number; y: number };
  onPos: (p: { x: number; y: number }) => void;
  onDismiss: (id: string) => void;
}) {
  const [size, setSize] = useState<{ w: number; h: number }>({ w: 620, h: 380 });
  const [z, setZ] = useState(1);
  const ref = useRef<HTMLDivElement>(null);

  // bring to front on focus
  const raise = () => setZ((z) => z + 1);

  const startDrag = (e: React.PointerEvent) => {
    e.preventDefault();
    raise();
    const sx = e.clientX, sy = e.clientY;
    const orig = { ...pos };
    const move = (ev: PointerEvent) => {
      onPos({ x: orig.x + (ev.clientX - sx), y: orig.y + (ev.clientY - sy) });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const startResize = (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    raise();
    const sx = e.clientX, sy = e.clientY;
    const orig = { ...size };
    const move = (ev: PointerEvent) => {
      setSize({ w: Math.max(320, orig.w + (ev.clientX - sx)), h: Math.max(200, orig.h + (ev.clientY - sy)) });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      ref={ref}
      className="tui-window"
      onPointerDown={raise}
      onClick={raise}
      style={{
        position: "absolute",
        left: 0,
        top: 0,
        transform: `translate(${pos.x}px, ${pos.y}px)`,
        width: size.w,
        height: size.h,
        zIndex: z,
        display: "flex",
        flexDirection: "column",
        background: "#0a0a14",
        border: "1px solid #232336",
        borderRadius: 6,
        boxShadow: "0 14px 40px rgba(0,0,0,0.5)",
        overflow: "hidden",
      }}
    >
      {/* title bar — drag handle */}
      <div
        onPointerDown={startDrag}
        className="row"
        style={{
          cursor: "grab",
          alignItems: "center",
          gap: 6,
          padding: "4px 8px",
          background: "#0e0e18",
          borderBottom: "1px solid #1e1e2e",
          userSelect: "none",
          flex: "none",
        }}
      >
        <span style={{ color: "var(--magenta)", fontSize: 10 }}>⠿</span>
        <span className="mono" style={{ fontSize: 10, color: "var(--dim2)", flex: 1 }}>
          opencode@{pane.dir.split("/").pop()}
        </span>
        <button
          className="ghost"
          style={{ fontSize: 10, padding: "2px 7px" }}
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => { void api.tuiStop(pane.id); onDismiss(pane.id); }}
        >
          ✕
        </button>
      </div>
      {/* terminal body */}
      <div style={{ flex: 1, minHeight: 0, position: "relative" }}>
        <TuiPane pane={pane} onExited={onDismiss} />
      </div>
      {/* resize handle — bottom-right */}
      <div
        onPointerDown={startResize}
        style={{
          position: "absolute",
          right: 0,
          bottom: 0,
          width: 16,
          height: 16,
          cursor: "nwse-resize",
          zIndex: 5,
        }}
      />
    </div>
  );
}
