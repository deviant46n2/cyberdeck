import type { BenchRow, PortMapSlot } from "../api";

/** One resident slot's most recent bench reading, for the chat header. */
export interface SlotBench {
  tps: number;
  ctx: number;
  model: string;
  at: number;
}

export const slotKey = (engine: string, port: number) => `${engine}:${port}`;

/** Latest bench reading per engine:port slot (rows arrive newest-any-order).
 * This is the "see the tok/s before you type" number for each resident. */
export function latestBySlot(rows: BenchRow[]): Map<string, SlotBench> {
  const latest = new Map<string, SlotBench>();
  for (const r of rows) {
    const prev = latest.get(slotKey(r.engine, r.port));
    if (!prev || r.at > prev.at) {
      latest.set(slotKey(r.engine, r.port), {
        tps: r.tps,
        ctx: r.ctx,
        model: r.model,
        at: r.at,
      });
    }
  }
  return latest;
}

/** State precedence for ordering: live slots first, then starting, then down. */
const STATE_RANK: Record<PortMapSlot["state"], number> = {
  up: 0,
  starting: 1,
  down: 2,
};

export function sortSlots(slots: PortMapSlot[]): PortMapSlot[] {
  return [...slots].sort((a, b) => STATE_RANK[a.state] - STATE_RANK[b.state]);
}
