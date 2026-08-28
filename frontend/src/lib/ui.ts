/**
 * Shared (non-DOM) UI helpers used across views.
 */

/** Badge class for a fit verdict. Shared by Browse and LoadoutEditor. */
export function verdictClass(v: string): string {
  if (v === "PASS") return "pass";
  if (v === "WARN") return "warn";
  return "oom";
}