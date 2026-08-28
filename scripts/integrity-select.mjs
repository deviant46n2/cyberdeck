// integrity-select.mjs — section-selection logic for the integrity engine.
//
// Ported from ModCanvas (s65) — section selection + --skip for platform-aware
// runs. Pure selection: given the section catalog and the requested
// names/skips, return the selected subset — or throw with available names.
//
// Semantics:
//   names []            → all sections (empty = no restriction)
//   skip  ['x']         → all except x (--skip=build-smoke)
//   names ['a','b']     → exactly a and b
//   unknown name/skip   → Error (loud, never a silent partial run)

export const SECTION_NAMES = [
  'line-limit',
  'artifact-hygiene',
  'stale-binary',
  'diff-hygiene',
  'doc-sync',
  'doc-anchors',
  'build-smoke',
  'suite-self',
]

export function selectSections(sections, names = [], skip = []) {
  const unknown = [...names, ...skip].filter((n) => !sections.some((s) => s.name === n))
  if (unknown.length > 0) {
    throw new Error(
      `unknown section(s) "${unknown.join('", "')}" (${SECTION_NAMES.join('|')})`,
    )
  }
  const selected = sections.filter(
    (s) => (names.length === 0 || names.includes(s.name)) && !skip.includes(s.name),
  )
  if (selected.length === 0) {
    throw new Error('all sections skipped — nothing to run')
  }
  return selected
}

export function parseArgs(args) {
  const seed = args.includes('--seed')
  const skip = args.filter((a) => a.startsWith('--skip=')).flatMap((a) => a.slice(7).split(',').filter(Boolean))
  const names = args.filter((a) => a !== '--seed' && !a.startsWith('--skip='))
  return { seed, names, skip }
}