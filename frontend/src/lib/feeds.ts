/** O4 recency-gate helpers for the FEEDS view.
 *
 * `feeds.last_seen` is persisted (audit-tracked) as a JSON string of an epoch
 * *seconds* value, so `settings_get` returns it double-quoted. Parse both the
 * quoted and plain forms, tolerating missing/invalid values by returning 0
 * ("nothing seen before"), so a fresh install shows everything as NEW instead
 * of nothing — the honest default for "what changed since I last looked".
 */
export const LAST_SEEN_KEY = "feeds.last_seen";

export function parseLastSeen(raw: string | null | undefined): number {
  if (!raw) return 0;
  const unquoted = raw.replace(/^"|"$/g, "");
  const n = Number(unquoted);
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
}

/** True when a catalog release (epoch-seconds `fetched_at`) entered after the
 * last time the user marked the lane seen. */
export function isNewSince(fetchedAtSecs: number, lastSeenSecs: number): boolean {
  return fetchedAtSecs > lastSeenSecs;
}