// integrity-git.mjs — git-aware checks for the integrity gate.
// diff-hygiene (whitespace lies about structure) and doc-sync (a commit that
// changes code but no doc is drift). Ported from ModCanvas (s22 split); the
// adapter-matrix check was ModCanvas-specific and is not ported.

import { execFileSync } from 'node:child_process'

export function checkDiffHygiene(rules, root) {
  const violations = []
  for (const args of [['diff', '--check'], ['diff', '--cached', '--check']]) {
    try {
      execFileSync('git', args, { cwd: root, encoding: 'utf8' })
    } catch (e) {
      const out = String(e.stdout ?? '')
      if (out.trim()) violations.push({ message: `git ${args.join(' ')}:\n${out.trim()}` })
      else throw new Error(`git ${args.join(' ')} failed: ${e.message}`)
    }
  }
  return { violations }
}

// Doc-sync: a commit that changes code but no doc is a drift CANDIDATE (the
// maintainer judges — pure refactors and reverts are legitimately doc-less).
// Surfaced, never a gate: false positives would break the gate's signal.
//
// Judged commits (rules.docSync.judgments, { commit, reason }) are reported as
// info, not candidates — the written reason retires them permanently. An
// unjudged candidate that ages out of the lookback window does NOT vanish: the
// health report's state file transitions it to visible debt (ModCanvas s30).
export function checkDocSync(rules, root) {
  const candidates = []
  const info = []
  const judged = new Map((rules.docSync.judgments ?? []).map((j) => [j.commit, j.reason]))
  let log
  try {
    log = execFileSync('git', ['log', `-${rules.docSync.lookback}`, '--format=%h', '--name-only'], {
      cwd: root,
      encoding: 'utf8',
    })
  } catch (e) {
    return { violations: [], candidates, info: [{ message: `no git history (${e.message})` }] }
  }
  const { codePaths, docPaths } = rules.docSync
  let commit = null
  const files = []
  const flush = () => {
    if (!commit) return
    const touchedCode = files.some((f) => codePaths.some((p) => f.startsWith(p)))
    const touchedDoc = files.some((f) => docPaths.some((p) => f.startsWith(p) || f === p))
    if (touchedCode && !touchedDoc) {
      const reason = judged.get(commit)
      const entry = {
        commit,
        files: files.filter((f) => codePaths.some((p) => f.startsWith(p))).slice(0, 5),
      }
      if (reason) info.push({ message: `commit ${commit} judged doc-less: ${reason}` })
      else candidates.push(entry)
    }
  }
  for (const line of log.split('\n')) {
    if (/^[0-9a-f]{7,40}$/.test(line.trim())) {
      flush()
      commit = line.trim()
      files.length = 0
    } else if (line.trim()) {
      files.push(line.trim())
    }
  }
  flush()
  return { violations: [], candidates, info }
}