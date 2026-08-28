#!/usr/bin/env node
// integrity-check.mjs — the repo's invariant catalog as executable checks.
//
// The engine here is GENERIC; the rules live in scripts/integrity-rules.json
// (data, not code). That split is the lift-out seam: a second project copies
// the engine and writes its own rules table — this repo inherited the design
// from ModCanvas. Rules are authoritative once the file exists; defaults below
// only apply before the first --seed.
//
// Sections (each maps to an AGENTS.md invariant):
//   line-limit       — no file > rules.lineLimit lines. Allowlist = parked
//                      debt with a written reason.
//   artifact-hygiene — no model blobs / .part / oversized archives in the tree,
//                      and no tracked secrets (git ls-files).
//   stale-binary     — the binary embeds the crates (and the frontend dist for
//                      release); a binary older than its newest source silently
//                      serves old code.
//   diff-hygiene     — git diff --check (whitespace lies about structure).
//   doc-sync         — code commit without a doc commit = drift CANDIDATE
//                      (maintainer judges; surfaced, never a gate).
//   doc-anchors      — content-level: doc mention ≠ code value is a violation.
//   build-smoke      — the frontend must actually build (tsc + vite).
//   suite-self       — the suite checks itself: doc↔package.json npm scripts,
//                      accepted entries cite a real doc, command descriptions.
//
// Usage (run from the repo root):
//   node scripts/integrity-check.mjs            # all sections; exit 1 on violations
//   node scripts/integrity-check.mjs --seed     # snapshot current tree into rules as parked
//   node scripts/integrity-check.mjs line-limit # one section
//   node scripts/integrity-check.mjs --skip=build-smoke        # all except named
//   # unknown section names error loudly (exit 2) — no silent partial runs.
// Tests: node --test scripts/integrity-select.test.mjs scripts/doc-sync-state.test.mjs
//
// Exit codes: 0 clean, 1 violations, 2 error (bad cwd, git failure).

import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'
import { pathToFileURL } from 'node:url'

import { RULES_PATH, loadRules, mergeRules } from './integrity-rules.mjs'
import { checkDiffHygiene, checkDocSync } from './integrity-git.mjs'
import { checkArtifactHygiene } from './integrity-artifacts.mjs'
import { checkDocAnchors } from './integrity-doc.mjs'
import { checkSuiteSelf } from './integrity-suite.mjs'
import { checkBuildSmoke } from './integrity-build.mjs'
import { selectSections, parseArgs } from './integrity-select.mjs'

// --- pure helpers ---------------------------------------------------------

export function walk(dir) {
  const out = []
  let entries
  try {
    entries = readdirSync(dir, { withFileTypes: true })
  } catch {
    return out // missing dir = no files
  }
  for (const e of entries) {
    const p = join(dir, e.name)
    if (e.isDirectory()) out.push(...walk(p))
    else out.push(p)
  }
  return out
}

// Editor-accurate line count: wc -l counts newlines, but a file without a
// trailing newline has one fewer \n (F9 fix from ModCanvas).
const lineCount = (file) => {
  const text = readFileSync(file, 'utf8')
  return (text.match(/\n/g) ?? []).length + (text.length > 0 && !text.endsWith('\n') ? 1 : 0)
}

// --- checks --------------------------------------------------------------

export function checkLineLimit(rules, root) {
  const violations = []
  const parked = []
  const accepted = []
  const candidates = []
  const hard = rules.lineLimitHard ?? rules.lineLimit * 2
  for (const dir of rules.lineLimitPaths) {
    for (const file of walk(join(root, dir))) {
      const lines = lineCount(file)
      if (lines <= rules.lineLimit) continue
      // Normalize to '/' separators so allowlists match on every OS.
      const rel = relative(root, file).split('\\').join('/')
      const entry = rules.allowlists['line-limit'].find((a) => a.path === rel)
      if (entry) {
        if (entry.kind === 'accepted') accepted.push({ path: rel, lines, reason: entry.reason })
        else parked.push({ path: rel, lines, reason: entry.reason, since: entry.since })
      } else if (lines > hard) {
        violations.push({ path: rel, lines })
      } else {
        candidates.push({
          path: rel,
          lines,
          message: `over ${rules.lineLimit}-line soft limit (${lines} lines) — needs a written PARKED/ACCEPTED reason`,
        })
      }
    }
  }
  return { violations, parked, accepted, candidates }
}

export function checkStaleBinary(rules, root) {
  const info = []
  const violations = []
  const parked = []
  const accepted = []
  for (const bin of rules.staleBinaries) {
    const abs = join(root, bin.path)
    if (!existsSync(abs)) {
      info.push({ message: `[${bin.name}] no binary at ${bin.path} — build first (npm run tauri dev / cargo build)` })
      continue
    }
    let newest = 0
    for (const dir of bin.sourcePaths) {
      for (const f of walk(join(root, dir))) {
        newest = Math.max(newest, statSync(f).mtimeMs)
      }
    }
    const binTime = statSync(abs).mtimeMs
    if (binTime < newest) {
      const entry = (rules.allowlists['stale-binary'] ?? []).find((a) => a.name === bin.name)
      if (entry) {
        if (entry.kind === 'accepted') accepted.push({ path: `[${bin.name}] ${bin.path}`, reason: entry.reason })
        else parked.push({ path: `[${bin.name}] ${bin.path}`, reason: entry.reason })
      } else {
        violations.push({
          message: `[${bin.name}] ${bin.path} (${new Date(binTime).toISOString()}) older than newest ${bin.sourcePaths.join(
            ' + ',
          )} source (${new Date(newest).toISOString()}) — STALE`,
        })
      }
    } else {
      info.push({ message: `[${bin.name}] binary newer than all sources` })
    }
  }
  return { violations, parked, accepted, info }
}

// --- reporting -----------------------------------------------------------

/** Render a candidate by shape: doc-sync candidates carry { commit, files };
 *  other sections carry { path } or { message }. Rendering every candidate as
 *  a doc-sync candidate printed "commit undefined" (ModCanvas s34). */
export function formatCandidate(c) {
  if (c.commit) {
    return `commit ${c.commit} changed code without docs: ${(c.files ?? []).join(', ')}`
  }
  return c.path ?? c.message ?? 'unknown candidate'
}

export function report(results) {
  let violationCount = 0
  for (const section of results) {
    const violations = section.violations ?? []
    const n = violations.length
    violationCount += n
    console.log(`\n== ${section.name} ==`)
    for (const v of section.violations) {
      console.log(`VIOLATION: ${v.path ?? v.message}${v.lines ? ` (${v.lines} lines)` : ''}`)
    }
    for (const c of section.candidates ?? []) console.log(`CANDIDATE: ${formatCandidate(c)}`)
    for (const p of section.parked ?? []) console.log(`PARKED:    ${p.path} — ${p.reason}`)
    for (const a of section.accepted ?? []) console.log(`ACCEPTED:  ${a.path} — ${a.reason}`)
    for (const i of section.info ?? []) console.log(`INFO:      ${i.message}`)
    if (n === 0 && !section.candidates?.length && !section.parked?.length && !section.accepted?.length && !section.info?.length) {
      console.log('  clean')
    }
  }
  console.log(`\n${violationCount} violation(s).`)
  return violationCount
}

// --- seeding -------------------------------------------------------------

export function seedRules(rulesPath, rules, root) {
  // Seed ONLY the allowlists. Never persist non-allowlist config: the
  // defaults supply it via mergeRules, and RegExp-bearing keys (docAnchors)
  // would be destroyed by JSON serialization (the s22 audit lesson).
  const fileRules = existsSync(rulesPath) ? JSON.parse(readFileSync(rulesPath, 'utf8')) : {}
  const base = mergeRules(rules, fileRules)
  const add = (key, list, reason) => {
    const cur = base.allowlists[key] ?? []
    const known = new Set(cur.map((a) => a.path ?? a.name))
    for (const v of list) {
      const id = v.path ?? v.name
      if (!known.has(id)) cur.push({ ...v, reason })
    }
    base.allowlists[key] = cur
  }
  const reasonLine = `pre-existing at tool introduction (${new Date().toISOString().slice(0, 10)}); parked — revisit on next touching change`
  add('line-limit', checkLineLimit(rules, root).violations, reasonLine)
  add('artifact-hygiene', checkArtifactHygiene(rules, root).violations, reasonLine)
  const next = { ...fileRules, allowlists: base.allowlists }
  writeFileSync(rulesPath, JSON.stringify(next, null, 2) + '\n')
  return next
}

// --- main ----------------------------------------------------------------

export function runAllSections(rules, root, names = [], skip = []) {
  const sections = [
    { name: 'line-limit', run: () => checkLineLimit(rules, root) },
    { name: 'artifact-hygiene', run: () => checkArtifactHygiene(rules, root) },
    { name: 'stale-binary', run: () => checkStaleBinary(rules, root) },
    { name: 'diff-hygiene', run: () => checkDiffHygiene(rules, root) },
    { name: 'doc-sync', run: () => checkDocSync(rules, root) },
    { name: 'doc-anchors', run: () => checkDocAnchors(rules, root) },
    { name: 'build-smoke', run: () => checkBuildSmoke(rules, root) },
    { name: 'suite-self', run: () => checkSuiteSelf(rules, root) },
  ]
  return selectSections(sections, names, skip).map((s) => ({ name: s.name, ...s.run() }))
}

function main() {
  const { seed, names, skip } = parseArgs(process.argv.slice(2))
  const root = process.cwd()
  if (!existsSync(join(root, 'src-tauri', 'src'))) {
    console.error('integrity-check: run from the repo root (no src-tauri/src here)')
    process.exit(2)
  }
  const rules = loadRules()

  if (seed) {
    seedRules(RULES_PATH, rules, root)
    console.log(`seeded ${RULES_PATH}`)
  }

  try {
    const results = runAllSections(rules, root, names, skip)
    process.exit(report(results) > 0 ? 1 : 0)
  } catch (e) {
    console.error(`integrity-check: ${e.message}`)
    process.exit(2)
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main()
}