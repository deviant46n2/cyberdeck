// doc-sync-state.mjs — the aged-out-candidate transition ledger (s30).
//
// The doc-sync check scans the last N commits; a candidate that scrolls out of
// the window silently vanishes from the report. That was the 65c1fe8 failure:
// the candidate aged out UNJUDGED and sat invisible for 5 sessions, its doc
// never written. This module makes the transition impossible to miss:
//
//   seen (last run, unjudged) + absent this run + still unjudged
//     -> "aged out unjudged" — surfaced as a P2 ledger-class item
//
// State is a gitignored JSON file (same pattern as .health-trend.json). A
// JUDGED candidate (rules.docSync.judgments) is removed from the state — the
// written judgment retires it permanently; a judged commit never re-flags.
//
// Usage (from health-report.mjs):
//   const state = loadDocSyncState(path)
//   const aged = transitionDocSync(state, seenCandidates, judgedCommits)
//   saveDocSyncState(state, path)

import { existsSync, readFileSync, writeFileSync } from 'node:fs'

const DEFAULT_STATE_PATH = '.doc-sync-state.json'

export function loadDocSyncState(path = DEFAULT_STATE_PATH) {
  try {
    const raw = JSON.parse(readFileSync(path, 'utf8'))
    return Array.isArray(raw) ? raw : []
  } catch {
    return []
  }
}

export function saveDocSyncState(state, path = DEFAULT_STATE_PATH) {
  writeFileSync(path, JSON.stringify(state, null, 2) + '\n')
}

// Returns aged-out-unjudged candidates (each: { commit, files, firstSeen }).
// Mutates `state` in place: drops judged commits, prunes nothing else (a
// candidate stays in the state until it either ages out or is judged).
export function transitionDocSync(state, currentCandidates, judgedCommits = []) {
  const judged = new Set(judgedCommits)
  const current = new Map(currentCandidates.map((c) => [c.commit, c.files]))
  const aged = []

  // Refresh: keep what's still current, remove what's judged.
  const kept = state.filter((s) => {
    if (judged.has(s.commit)) return false
    if (current.has(s.commit)) {
      // still in-window and still unjudged — carry it forward
      return true
    }
    // was seen last run, not in-window this run, not judged -> aged out
    aged.push({ commit: s.commit, files: s.files, firstSeen: s.firstSeen })
    return false
  })

  // New candidates enter the state (firstSeen = today, kept minimal).
  const seen = new Set(kept.map((s) => s.commit))
  const today = new Date().toISOString().slice(0, 10)
  for (const c of currentCandidates) {
    if (!seen.has(c.commit) && !judged.has(c.commit)) {
      kept.push({ commit: c.commit, files: c.files, firstSeen: today })
    }
  }

  state.length = 0
  state.push(...kept)
  return aged
}
