// integrity-rules.mjs — the rules/config layer of the integrity gate.
//
// Ported from ModCanvas and adapted for cyberdeck's workspace shape. The
// on-disk rules file (scripts/integrity-rules.json) is authoritative once it
// exists; the defaults here fill any gaps via mergeRules — a stale rules file
// must never crash the gate.

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

export const RULES_PATH = join(process.cwd(), 'scripts', 'integrity-rules.json')

export const DEFAULT_RULES = {
  // Line-count policy: 300 is the advisory heuristic, not a law — cohesion >
  // arbitrary count (AGENTS.md "File & Function Size Limits"). Files over the
  // soft limit need a written PARKED/ACCEPTED reason (surfaced as candidates);
  // only files over the HARD limit fail the gate and get parked via --seed.
  lineLimit: 300,
  lineLimitHard: 600,
  lineLimitPaths: ['src-tauri/src', 'crates', 'frontend/src', 'scripts'],
  // Artifact hygiene scans these dirs for model blobs / partial downloads and
  // (git-wide) for tracked secrets. `crates` includes Cargo.tomls — fine, the
  // blob/secret patterns never match config.
  artifactDirs: ['src-tauri', 'crates', 'frontend/src', 'frontend/public', 'scripts', '.github'],
  // Per-binary staleness (the Tauri binary embeds the workspace crates; the
  // CLI embeds the cores; the release binary ALSO embeds frontend/src via the
  // bundled dist). Dev builds hot-reload the frontend through Vite, so frontend
  // edits must NOT flag the dev binary (F4 pattern from ModCanvas).
  staleBinaries: [
    {
      name: 'tauri-dev',
      path: 'target/debug/cyberdeck',
      sourcePaths: ['src-tauri/src', 'crates/deck-core/src', 'crates/deck-engines/src', 'crates/deck-feeds/src', 'crates/deck-tauri/src'],
    },
    {
      name: 'tauri-release',
      path: 'target/release/cyberdeck',
      sourcePaths: ['src-tauri/src', 'crates/deck-core/src', 'crates/deck-engines/src', 'crates/deck-feeds/src', 'crates/deck-tauri/src', 'frontend/src'],
    },
    {
      name: 'deck-cli',
      path: 'target/debug/deck',
      sourcePaths: ['crates/deck-cli/src', 'crates/deck-core/src', 'crates/deck-engines/src', 'crates/deck-feeds/src'],
    },
  ],
  docSync: {
    codePaths: ['src-tauri/src', 'crates', 'frontend/src'],
    docPaths: ['feature-parity.md', 'README.md', 'AGENTS.md'],
    lookback: 10,
    // A doc-sync candidate the maintainer has JUDGED — legitimately doc-less
    // (pure refactor/revert/test-only) or its docs written elsewhere. The
    // written reason retires it permanently (s30 lesson from ModCanvas).
    judgments: [],
  },
  suiteSelf: {
    commandsDir: '.opencode/command',
    skillsDir: '.opencode/skills',
    docsFiles: ['AGENTS.md', 'feature-parity.md', 'README.md'],
    packageJson: 'package.json',
  },
  buildSmoke: {},
  // Doc anchors are code literals (RegExp), never JSON — the seed pass
  // persists allowlists only, so these survive (the s22 audit lesson).
  docAnchors: [
    {
      name: 'app-version',
      codeFile: 'src-tauri/tauri.conf.json',
      codePattern: /"version":\s*"(\d+\.\d+\.\d+)"/,
      docFile: 'AGENTS.md',
      docPattern: /cyberdeck v(\d+\.\d+\.\d+)/g,
    },
  ],
  allowlists: {
    'line-limit': [],
    'artifact-hygiene': [],
    'stale-binary': [],
  },
}

// The on-disk rules file is authoritative once it exists, but the engine may
// gain config keys later. Merge: loaded rules overlay the defaults, and
// defaults fill gaps — a stale rules file must never crash the gate.
export function mergeRules(base, over) {
  return {
    ...base,
    ...over,
    allowlists: { ...base.allowlists, ...(over.allowlists ?? {}) },
  }
}

// Doc-sync judgments live in their own file (scripts/doc-sync-judgments.json)
// so integrity-rules.json stays small (ModCanvas s32 lesson).
export const JUDGMENTS_PATH = join(process.cwd(), 'scripts', 'doc-sync-judgments.json')

export const loadRules = () => {
  const base = existsSync(RULES_PATH)
    ? mergeRules(DEFAULT_RULES, JSON.parse(readFileSync(RULES_PATH, 'utf8')))
    : DEFAULT_RULES
  if (!existsSync(JUDGMENTS_PATH)) return base
  const extra = JSON.parse(readFileSync(JUDGMENTS_PATH, 'utf8'))
  return {
    ...base,
    docSync: {
      ...(base.docSync ?? {}),
      judgments: [...(base.docSync?.judgments ?? []), ...(extra.judgments ?? [])],
    },
  }
}