// integrity-doc.mjs — doc-vs-code consistency anchors for the integrity gate.
//
// The commit-level doc-sync check (integrity-git.mjs) catches *new* drift;
// this one catches *content* drift: specific facts that must agree between a
// doc and the code it describes (app version, model-fit numbers, ports...).
// Anchors are data in the rules file — new anchor = new rules entry, never an
// edit to this engine. Ported from ModCanvas.

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

export function checkDocAnchors(rules, root) {
  const violations = []
  const info = []
  for (const anchor of rules.docAnchors ?? []) {
    const codeFile = join(root, anchor.codeFile)
    const docFile = join(root, anchor.docFile)
    if (!existsSync(codeFile) || !existsSync(docFile)) {
      info.push({ message: `anchor ${anchor.name}: missing file (code=${anchor.codeFile} doc=${anchor.docFile}) — update rules` })
      continue
    }
    const code = readFileSync(codeFile, 'utf8')
    const doc = readFileSync(docFile, 'utf8')
    // Normalize patterns from rules data: RegExp literals or JSON strings.
    // Reconstructing from .source must preserve the literal's flags (e.g.
    // /m on ^version) — and dedupe 'g' (duplicate flags throw).
    const withG = (pattern) => {
      const src = pattern.source ?? String(pattern)
      const flags = new Set([...(pattern.flags ?? ''), 'g'])
      return new RegExp(src, [...flags].join(''))
    }
    // A pattern that throws (broken string regex in hand-edited rules) or
    // produces no capture group must FAIL LOUDLY, never pass vacuously (the
    // s22 audit found a corrupted {} pattern matching "[object Object]").
    let codeRe, docRe
    try {
      codeRe = withG(anchor.codePattern)
      docRe = withG(anchor.docPattern)
    } catch (e) {
      violations.push({ path: `anchor ${anchor.name}: bad pattern in rules (${e.message}) — fix the rules` })
      continue
    }
    const codeValues = [...code.matchAll(codeRe)].map((m) => m[1])
    if (codeValues.some((v) => v === undefined)) {
      violations.push({ path: `anchor ${anchor.name}: pattern matches without a capture group (${anchor.codeFile}) — fix the rules, it would pass vacuously` })
      continue
    }
    if (!codeValues.length) {
      info.push({ message: `anchor ${anchor.name}: pattern not found in code (${anchor.codeFile}) — update rules` })
      continue
    }
    const unique = [...new Set(codeValues)]
    if (unique.length > 1) {
      violations.push({
        path: `anchor ${anchor.name}: multiple values in code (${unique.join(', ')} at ${anchor.codeFile}) — update rules to disambiguate`,
      })
      continue
    }
    const codeValue = unique[0]
    const docValues = [...doc.matchAll(docRe)].map((m) => m[1])
    if (docValues.some((v) => v === undefined)) {
      violations.push({ path: `anchor ${anchor.name}: doc pattern matches without a capture group (${anchor.docFile}) — fix the rules, it would pass vacuously` })
      continue
    }
    if (!docValues.length) {
      info.push({ message: `anchor ${anchor.name}: doc (${anchor.docFile}) does not mention it — verify this is intended` })
      continue
    }
    const stale = docValues.filter((v) => v !== codeValue)
    for (const v of stale) {
      violations.push({ path: `${anchor.docFile} mentions ${anchor.name}="${v}" but ${anchor.codeFile} has "${codeValue}"` })
    }
  }
  return { violations, info }
}