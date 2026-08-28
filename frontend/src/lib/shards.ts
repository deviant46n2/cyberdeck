/**
 * Shard-set grouping for multi-part GGUF/safetensors repos.
 *
 * Given any member of a split archive (`-NNNNN-of-MMMMM.gguf` /
 * `-NNNNN-of-MMMMM.safetensors`) and the full sibling list, returns every
 * shard of that set in ascending part order. Single files (no shard suffix)
 * return themselves. A set missing any shard is NOT queueable — it falls back
 * to just `chosen` rather than queueing a set the scanner can't open.
 *
 * The vault only indexes a shard set once every member has landed, so the
 * download manager queues/relates shards via this grouping.
 */

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