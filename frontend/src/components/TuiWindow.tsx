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
  selected,
  onSelect,
  onContextMenu,
  role,
  onStartConnect,
  connecting,
  zoom,
}: {
  pane: { id: string; dir: string };
  pos: { x: number; y: number };
  onPos: (p: { x: number; y: number }) => void;
  onDismiss: (id: string) => void;
  selected?: boolean;
  onSelect?: (id: string) => void;
  onContextMenu?: (e: React.MouseEvent, id: string) => void;
  role?: string;
  onStartConnect?: (id: string) => void;
  connecting?: boolean;
  zoom?: number;
}) {
  const [size, setSize] = useState<{ w: number; h: number }>({ w: 780, h: 480 });
  const [z, setZ] = useState(1);
  const ref = useRef<HTMLDivElement>(null);
  const zoomRef = useRef(zoom ?? 1);
  zoomRef.current = zoom ?? 1;

  // bring to front on focus
  const raise = () => {
    setZ((z) => z + 1);
    onSelect?.(pane.id);
  };

  const startDrag = (e: React.PointerEvent) => {
    e.preventDefault();
    raise();
    const sx = e.clientX, sy = e.clientY;
    const orig = { ...pos };
    const move = (ev: PointerEvent) => {
      const z = zoomRef.current;
      onPos({ x: orig.x + (ev.clientX - sx) / z, y: orig.y + (ev.clientY - sy) / z });
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
      const z = zoomRef.current;
      setSize({ w: Math.max(320, orig.w + (ev.clientX - sx) / z), h: Math.max(200, orig.h + (ev.clientY - sy) / z) });
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
      onContextMenu={(e) => { e.preventDefault(); onContextMenu?.(e, pane.id); }}
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
        background: "#0b0b0b",
        border: `1px solid ${selected ? "var(--magenta)" : "var(--line)"}`,
        boxShadow: selected ? "0 0 0 2px rgba(210,153,34,0.25)" : "none",
        overflow: "hidden",
      }}
    >
      {/* plain drag bar — role badge + loop handle · click selects for drawer */}
      <div
        onPointerDown={(e) => { onSelect?.(pane.id); startDrag(e); }}
        onClick={() => onSelect?.(pane.id)}
        title={`opencode ${pane.id} — drag to move, click to select${role ? ` · role: ${role}` : ""}`}
        style={{
          height: 18,
          flex: "none",
          cursor: "grab",
          background: selected ? "rgba(210,153,34,0.18)" : "rgba(255,255,255,0.03)",
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "0 6px",
          fontSize: 9,
          color: "var(--dim2)",
        }}
      >
        <span onPointerDown={(e) => e.stopPropagation()} onClick={(e) => { e.stopPropagation(); onSelect?.(pane.id); }} style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", cursor: "pointer" }}>{role ? `◉ ${role} — click to edit` : "◯ no role — click to assign"}</span>
        <span
          onPointerDown={(e) => { e.stopPropagation(); onStartConnect?.(pane.id); }}
          title={connecting ? "pick target terminal to connect (loop)" : "drag to connect — loopable edge"}
          style={{
            width: 10, height: 10, borderRadius: "50%",
            background: connecting ? "var(--magenta)" : "var(--line)",
            border: "1px solid var(--line-bright)",
            cursor: "crosshair",
            flex: "none",
          }}
        />
      </div>
      {/* terminal body — stock opencode TUI, no wrapper */}
      <div style={{ flex: 1, minHeight: 0, position: "relative" }} onPointerDown={(e) => e.stopPropagation()}>
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
      {/* tiny chrome-free dismiss — bottom edge hover */}
      <button
        title="close"
        onPointerDown={(e) => e.stopPropagation()}
        onClick={() => { void api.tuiStop(pane.id); onDismiss(pane.id); }}
        style={{
          position: "absolute",
          top: 6,
          right: 4,
          fontSize: 9,
          lineHeight: 1,
          padding: "2px 4px",
          background: "rgba(0,0,0,0.55)",
          border: "1px solid var(--line)",
          color: "var(--dim2)",
          opacity: 0.6,
        }}
      >
        ✕
      </button>
    </div>
  );
}
