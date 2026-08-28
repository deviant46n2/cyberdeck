#!/usr/bin/env node
// health-report.mjs — repo health thermometer (the debt ledger).
//
// Ported from ModCanvas. Score = "manageable debt load", NOT code quality.
// 100 = ledger empty + suite clean. Deductions:
//   violation  — full weight: a broken invariant (the suite's exit-1 class)
//   candidate  — partial: surfaced, needs maintainer judgment (doc-sync)
//   parked     — near-zero: known debt WITH a written reason (good parks must
//                not be punished)
//   ledger item — weight by its explicit priority field (data, not magic)
//
// Failure classes are DATA, not code: new classes = new rows in the rules
// file's `health` block. Report-only: exit 0 always (no gate — a report that
// gates inherits the doc-sync false-positive politics).
//
// Trend: appended to a gitignored .health-trend.json (visible to us, never
// committed — the history lives in the file, not in git churn).
//
// Usage: node scripts/health-report.mjs   (run from the repo root)

import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { execFileSync } from 'node:child_process'

import { loadRules } from './integrity-rules.mjs'
import { runAllSections } from './integrity-check.mjs'
import { loadDocSyncState, saveDocSyncState, transitionDocSync } from './doc-sync-state.mjs'

const REPO_NAME = 'cyberdeck'
const DEFAULT_TREND_PATH = join(process.cwd(), '.health-trend.json')
// Lazy: tests override TREND_PATH via env after import; the constant would
// freeze it at module load.
const trendPath = () => process.env.TREND_PATH ?? DEFAULT_TREND_PATH

const HEALTH_RULES_PATH = join(process.cwd(), 'scripts', 'health-rules.json')

const DEFAULT_HEALTH = {
  weights: { violation: 10, candidate: 3, parked: 0.5 },
  parkedWeights: {},
  ledger: [],
}

// Health rules live in their own data file. New failure classes = new ledger
// entries HERE, never a script edit.
export function loadHealthRules() {
  try {
    const over = JSON.parse(readFileSync(HEALTH_RULES_PATH, 'utf8'))
    return {
      weights: { ...DEFAULT_HEALTH.weights, ...(over.weights ?? {}) },
      parkedWeights: { ...DEFAULT_HEALTH.parkedWeights, ...(over.parkedWeights ?? {}) },
      ledger: over.ledger ?? [],
    }
  } catch {
    return DEFAULT_HEALTH
  }
}

/** Distinguish debt from accepted intentional decisions (s36): a score below
 * 100 with zero debt must say so plainly instead of leaving the number
 * unexplained. */
export function knownDebtLine(totalDebt, totalAccepted, deduction) {
  if (totalDebt > 0) return `Known debt: ${totalDebt} item(s).`
  if (totalAccepted > 0) {
    return deduction > 0
      ? `Known debt: 0 — the remaining ${deduction} point(s) are ${totalAccepted} accepted intentional decision(s), not debt.`
      : `Known debt: 0 — ${totalAccepted} accepted intentional decision(s), not debt.`
  }
  return 'Known debt: 0.'
}
const MAX_TREND = 120

const WEIGHT_DEFAULTS = { violation: 10, candidate: 3, parked: 0.5 }
const PRIORITY_WEIGHTS = { P0: 15, P1: 10, P2: 5, P3: 2 }

// Local calendar date (YYYY-MM-DD): toISOString is UTC and rolls over in the
// evening; a run at 19:xx would be logged as tomorrow.
const localDate = () => {
  const d = new Date()
  const p = (n) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

function countOf(results, name, kind) {
  return results.find((r) => r.name === name)?.[kind]?.length ?? 0
}

// Parked items default to the flat parked weight, but a section may define
// SEVERITY BANDS (health.parkedWeights[section] = [{min, max, weight}]): the
// parked entry's own metric (e.g. line-limit's `lines`) picks a band. This is
// how a 1800-line deck-tauri/src/lib.rs costs more than a 320-line file.
export function parkedWeightFor(health, section, entry, fallback) {
  const bands = health.parkedWeights?.[section] ?? []
  if (bands.length === 0 || entry.lines == null) return fallback
  for (const b of bands) {
    if (entry.lines >= b.min && (b.max == null || entry.lines <= b.max)) return b.weight
  }
  return fallback
}

export function computeScore(results, health) {
  const weights = { ...WEIGHT_DEFAULTS, ...(health.weights ?? {}) }
  const breakdown = {}
  let deduction = 0
  for (const section of results) {
    const nV = countOf(results, section.name, 'violations')
    const nC = countOf(results, section.name, 'candidates') ?? 0
    const nP = countOf(results, section.name, 'parked') ?? 0
    const nA = countOf(results, section.name, 'accepted') ?? 0
    let parkedDeduction = 0
    for (const p of section.parked ?? []) {
      parkedDeduction += parkedWeightFor(health, section.name, p, weights.parked)
    }
    const d = nV * weights.violation + nC * weights.candidate + parkedDeduction
    breakdown[section.name] = { violations: nV, candidates: nC, parked: nP, accepted: nA, deduction: +d.toFixed(1) }
    deduction += d
  }
  for (const item of health.ledger ?? []) {
    const w = PRIORITY_WEIGHTS[item.priority] ?? 5
    breakdown[`ledger:${item.id}`] = { violations: 0, candidates: 0, parked: 0, accepted: 0, deduction: w }
    deduction += w
  }
  return { score: Math.max(0, Math.round(100 - deduction)), breakdown, deduction: +deduction.toFixed(1) }
}

/** Last commit timestamp of a path (ISO-8601 local), or null when the path is
 * untracked/absent. Git is truth: a park's tripwire ("revisit on next touching
 * change") fires only when the file was ACTUALLY touched after it was parked. */
function lastCommitAt(root, path) {
  try {
    return execFileSync('git', ['log', '-1', '--format=%cI', '--', path], {
      cwd: root,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
  } catch {
    return null
  }
}

function touchedSince(since, commitAt) {
  if (!since || !commitAt) return false
  return commitAt > since // ISO-8601 strings compare lexicographically
}

export function rankWork(results, health, root) {
  // What to work on next: new violations first, then ledger items, then
  // candidates, then parked items whose tripwire has FIRED.
  const out = []
  for (const section of results) {
    for (const v of section.violations ?? []) {
      out.push({ class: 'violation', section: section.name, name: v.path ?? v.message, priority: 'P0' })
    }
  }
  for (const item of health.ledger ?? []) {
    out.push({ class: 'ledger', section: 'ledger', name: item.id, priority: item.priority, reason: item.reason })
  }
  for (const section of results) {
    for (const c of section.candidates ?? []) {
      const name = c.commit ? `commit ${c.commit} changed code without docs: ${(c.files ?? []).join(', ')}` : (c.path ?? c.message)
      out.push({ class: 'candidate', section: section.name, name, priority: 'P2' })
    }
  }
  for (const section of results) {
    for (const p of section.parked ?? []) {
      if (!/next touching change/.test(p.reason ?? '')) continue
      const commitAt = lastCommitAt(root, p.path)
      if (touchedSince(p.since, commitAt)) {
        out.push({
          class: 'parked-tripwire',
          section: section.name,
          name: p.path,
          priority: 'P3',
          reason: `${p.reason}; touched ${commitAt} — revisit`,
        })
      }
    }
  }
  const order = { P0: 0, P1: 1, P2: 2, P3: 3 }
  return out.sort((a, b) => order[a.priority] - order[b.priority])
}

export function loadTrend() {
  try {
    return JSON.parse(readFileSync(trendPath(), 'utf8'))
  } catch {
    return []
  }
}

export function appendTrend(entry) {
  const trend = loadTrend()
  if (trend.some((t) => t.date === entry.date)) return trend // one per day
  trend.push(entry)
  const trimmed = trend.slice(-MAX_TREND)
  writeFileSync(trendPath(), JSON.stringify(trimmed, null, 2) + '\n')
  return trimmed
}

function printReport() {
  const root = process.cwd()
  if (!existsSync(join(root, 'src-tauri', 'src'))) {
    console.error('health-report: run from the repo root')
    process.exit(2)
  }
  const rules = loadRules()
  const health = loadHealthRules()
  const results = runAllSections(rules, root)
  const { score, breakdown, deduction } = computeScore(results, health)

  // Aged-out doc-sync candidates must not vanish (ModCanvas 65c1fe8 class):
  // transition them into visible P2 work; a judged candidate is retired.
  const statePath = process.env.DOC_SYNC_STATE_PATH ?? join(root, '.doc-sync-state.json')
  const docSyncState = loadDocSyncState(statePath)
  const docSyncResults = results.find((r) => r.name === 'doc-sync')
  const seen = docSyncResults?.candidates ?? []
  const judged = (rules.docSync.judgments ?? []).map((j) => j.commit)
  const agedOut = transitionDocSync(docSyncState, seen, judged)
  saveDocSyncState(docSyncState, statePath)

  const work = rankWork(results, health, root)
  for (const a of agedOut) {
    const files = (a.files ?? []).join(', ')
    work.push({
      class: 'aged-out-unjudged',
      section: 'doc-sync',
      name: `commit ${a.commit} changed code without docs and aged out UNJUDGED (first seen ${a.firstSeen}): ${files}`,
      priority: 'P2',
      reason: 'judge it now: doc-less (write the reason in docSync.judgments) or write the docs it needed',
    })
  }
  work.sort((a, b) => ({ P0: 0, P1: 1, P2: 2, P3: 3 })[a.priority] - ({ P0: 0, P1: 1, P2: 2, P3: 3 })[b.priority])

  const totalDebt = results.reduce(
    (n, s) => n + (s.violations?.length ?? 0) + (s.candidates?.length ?? 0) + (s.parked?.length ?? 0),
    0,
  )
  const totalAccepted = results.reduce((n, s) => n + (s.accepted?.length ?? 0), 0)

  console.log(`\n${REPO_NAME} repo health: ${score}/100 (manageable debt load)`)
  console.log(`  deductions: ${deduction} points`)
  for (const [section, b] of Object.entries(breakdown)) {
    if (b.deduction === 0 && b.violations === 0 && b.candidates === 0 && b.parked === 0 && b.accepted === 0) continue
    console.log(
      `  ${section.padEnd(28)} V:${b.violations} C:${b.candidates} P:${b.parked} A:${b.accepted}  −${b.deduction}`,
    )
  }
  console.log(knownDebtLine(totalDebt, totalAccepted, deduction))

  if (work.length > 0) {
    console.log(`\nWhat to work on next (${work.length}):`)
    for (const w of work) {
      const reason = w.reason ? ` — ${w.reason}` : ''
      console.log(`  [${w.priority}] ${w.class}: ${w.name}${reason}`)
    }
  } else {
    console.log('\nWhat to work on next: nothing. Ledger empty, suite clean.')
  }

  const trend = appendTrend({
    date: localDate(),
    score,
    deduction: +deduction.toFixed(1),
    workCount: work.length,
  })
  if (trend.length > 1) {
    console.log('\nTrend (last run vs previous):')
    const prev = trend[trend.length - 2]
    const delta = score - prev.score
    console.log(`  ${prev.date}: ${prev.score} → ${new Date().toISOString().slice(0, 10)}: ${score} (${delta >= 0 ? '+' : ''}${delta})`)
  } else {
    console.log('\nTrend: first run — baseline recorded.')
  }
  // Report-only: never gate.
  process.exit(0)
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  printReport()
}