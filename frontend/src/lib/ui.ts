/**
 * Shared (non-DOM) UI helpers used across views.
 */

/** Badge class for a fit verdict. Shared by Market and LoadoutEditor. */
export function verdictClass(v: string): string {
  if (v === "PASS") return "pass";
  if (v === "WARN") return "warn";
  return "oom";
}

/**
 * Matches auxiliary GGUF files that are NOT model weights — mmproj,
 * imatrix, embedding adapters, etc.  Used to exclude them from
 * "smallest quant" / DISK-column calculations.
 */
export const AUX_GGUF =
  /(?:mmproj|imatrix|embed|babble|clip|vision_tower|text_encoder|tokenizer|added_tokens|chat_template|probs)\b/i;