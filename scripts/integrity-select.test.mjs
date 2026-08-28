// integrity-select.test.mjs — section-selection + CLI arg parsing.
//
// Ported from ModCanvas (s65) with the cyberdeck section list. These tests
// lock the selection semantics so a future CLI change cannot silently narrow
// or widen what a platform-aware run executes.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { selectSections, parseArgs, SECTION_NAMES } from './integrity-select.mjs'

const sections = SECTION_NAMES.map((name) => ({ name, run: () => null }))

test('selectSections: no names, no skip → all sections', () => {
  const sel = selectSections(sections)
  assert.deepEqual(sel.map((s) => s.name), SECTION_NAMES)
})

test('selectSections: skip build-smoke → all except it (platform-aware shape)', () => {
  const sel = selectSections(sections, [], ['build-smoke'])
  assert.ok(!sel.some((s) => s.name === 'build-smoke'))
  assert.equal(sel.length, SECTION_NAMES.length - 1)
})

test('selectSections: named sections run exactly those', () => {
  const sel = selectSections(sections, ['line-limit', 'artifact-hygiene'])
  assert.deepEqual(sel.map((s) => s.name), ['line-limit', 'artifact-hygiene'])
})

test('selectSections: skip can be comma-listed by parseArgs (multi-section skip)', () => {
  const { skip } = parseArgs(['--skip=build-smoke,suite-self'])
  assert.deepEqual(skip, ['build-smoke', 'suite-self'])
  const sel = selectSections(sections, [], skip)
  assert.deepEqual(
    sel.map((s) => s.name),
    SECTION_NAMES.filter((n) => !skip.includes(n)),
  )
})

test('selectSections: unknown name errors loudly (never a silent partial run)', () => {
  assert.throws(() => selectSections(sections, ['bogus']), /unknown section/)
  assert.throws(() => selectSections(sections, [], ['bogus']), /unknown section/)
})

test('selectSections: everything skipped errors, does not return empty', () => {
  assert.throws(() => selectSections(sections, [], [...SECTION_NAMES]), /nothing to run/)
})

test('parseArgs: no args → all sections, no skip', () => {
  assert.deepEqual(parseArgs([]), { seed: false, names: [], skip: [] })
})

test('parseArgs: --seed alone is the seed flag, not a section name', () => {
  const { seed, names } = parseArgs(['--seed'])
  assert.equal(seed, true)
  assert.deepEqual(names, [])
})

test('parseArgs: mixed named sections + skip + seed parse independently', () => {
  const { seed, names, skip } = parseArgs(['--seed', 'line-limit', '--skip=build-smoke'])
  assert.equal(seed, true)
  assert.deepEqual(names, ['line-limit'])
  assert.deepEqual(skip, ['build-smoke'])
})

test('parseArgs: empty --skip= value contributes nothing', () => {
  const { skip } = parseArgs(['--skip='])
  assert.deepEqual(skip, [])
})