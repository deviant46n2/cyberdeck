// doc-sync-state.test.mjs — the aged-out-candidate transition ledger.
// Ported from ModCanvas (s30 lesson). Run:
//   node --test scripts/doc-sync-state.test.mjs

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { loadDocSyncState, saveDocSyncState, transitionDocSync } from './doc-sync-state.mjs'

const fixturePath = () => join(mkdtempSync(join(tmpdir(), 'doc-sync-state-')), 'state.json')

test('transition: an unjudged candidate that ages out is returned as aged-out', () => {
  const state = [{ commit: 'aaaa111', files: ['src/a.rs'], firstSeen: '2026-08-01' }]
  const aged = transitionDocSync(state, [], [])
  assert.equal(aged.length, 1)
  assert.equal(aged[0].commit, 'aaaa111')
  assert.equal(aged[0].firstSeen, '2026-08-01')
  assert.deepEqual(state, [], 'aged-out candidate leaves the state')
})

test('transition: an in-window unjudged candidate is carried forward, not aged', () => {
  const state = [{ commit: 'bbbb222', files: ['src/b.rs'], firstSeen: '2026-08-01' }]
  const aged = transitionDocSync(state, [{ commit: 'bbbb222', files: ['src/b.rs'] }], [])
  assert.equal(aged.length, 0)
  assert.equal(state.length, 1)
  assert.equal(state[0].firstSeen, '2026-08-01', 'firstSeen is preserved, not reset')
})

test('transition: a judged candidate is retired permanently, never aged', () => {
  const state = [{ commit: 'cccc333', files: ['src/c.rs'], firstSeen: '2026-08-01' }]
  const aged = transitionDocSync(state, [], ['cccc333'])
  assert.equal(aged.length, 0, 'a judged commit is not surfaced as aged-out')
  assert.deepEqual(state, [], 'judged commit leaves the state')
})

test('transition: new candidates enter the state with firstSeen = today', () => {
  const state = []
  const aged = transitionDocSync(state, [{ commit: 'dddd444', files: ['src/d.rs'] }], [])
  assert.equal(aged.length, 0)
  assert.equal(state.length, 1)
  assert.match(state[0].firstSeen, /^\d{4}-\d{2}-\d{2}$/)
})

test('state file: round-trips through disk', () => {
  const path = fixturePath()
  saveDocSyncState([{ commit: 'eeee555', files: ['src/e.rs'], firstSeen: '2026-08-01' }], path)
  const loaded = loadDocSyncState(path)
  assert.equal(loaded.length, 1)
  assert.equal(loaded[0].commit, 'eeee555')
  assert.match(readFileSync(path, 'utf8'), /eeee555/)
})

test('state file: missing/corrupt file loads as empty, never throws', () => {
  const path = fixturePath()
  assert.deepEqual(loadDocSyncState(path), [])
  const corrupt = join(mkdtempSync(join(tmpdir(), 'doc-sync-state-')), 'bad.json')
  writeFileSync(corrupt, '{not json')
  assert.deepEqual(loadDocSyncState(corrupt), [])
})