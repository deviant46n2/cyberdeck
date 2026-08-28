// integrity-suite.mjs — the suite checking ITSELF (the s13 meta-rule: the
// tooling is a maintainership artifact with the same failure classes — doc
// drift, stale references, dead scripts). Ported from ModCanvas, adapted to
// this repo's npm layout (cyberdeck has no frontend test runner, so the
// test-count cross-check is omitted).

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

export function checkSuiteSelf(rules, root) {
  const violations = []
  const candidates = []
  const info = []
  const { commandsDir, skillsDir, docsFiles, packageJson } = rules.suiteSelf

  // 1. Every command carries a description in frontmatter (agent: tutor was
  //    ModCanvas-specific; here any agent is fine, the description is not).
  const commandsDirAbs = join(root, commandsDir)
  if (existsSync(commandsDirAbs)) {
    for (const file of readdirSync(commandsDirAbs).filter((f) => f.endsWith('.md'))) {
      const fm = frontmatter(readFileSync(join(commandsDirAbs, file), 'utf8'))
      if (!fm.description) violations.push({ path: `${commandsDir}/${file}: missing description` })
    }
  }

  // 2. Every skill a command references actually exists.
  const skillsDirAbs = join(root, skillsDir)
  if (existsSync(commandsDirAbs) && existsSync(skillsDirAbs)) {
    const commandText = readdirSync(commandsDirAbs)
      .filter((f) => f.endsWith('.md'))
      .map((f) => readFileSync(join(commandsDirAbs, f), 'utf8'))
      .join('\n')
    for (const name of commandText.matchAll(/tutor-[\w-]+/g)) {
      const skill = name[0]
      if (!existsSync(join(skillsDirAbs, skill, 'SKILL.md'))) {
        violations.push({ path: `${commandsDir} references missing skill ${skill}/SKILL.md` })
      }
    }
  }

  // 3. npm script claims in docs must exist. Backticked references are
  //    unambiguous — a missing script is a VIOLATION. Plain references are
  //    ambiguous prose — they surface as CANDIDATES. Resolution order:
  //    inline `--prefix <dir>` → trailing "(in `dir/`)" marker → root.
  const pkgPath = join(root, packageJson)
  const NPM_VERBS = new Set([
    'install', 'i', 'add', 'remove', 'rm', 'update', 'up', 'dlx', 'exec', 'init', 'create',
    'link', 'unlink', 'prune', 'pack', 'publish', 'root', 'bin', 'env', 'config', 'help',
    'ls', 'list', 'why', 'outdated', '-v', '--version', 'rebuild', 'audit', 'dedupe',
  ])
  if (existsSync(pkgPath)) {
    const pkg = safeJson(readFileSync(pkgPath, 'utf8'))
    const scriptsRoot = pkg?.scripts ?? {}
    const scriptsFor = (dir) => {
      if (!dir) return scriptsRoot
      const p = join(root, dir, 'package.json')
      return existsSync(p) ? safeJson(readFileSync(p, 'utf8'))?.scripts ?? null : null
    }
    for (const docsFile of docsFiles) {
      const docsPath = join(root, docsFile)
      if (!existsSync(docsPath)) continue
      const docs = readFileSync(docsPath, 'utf8')

      // Backticked `npm ...` tokens: extract the script name (first token
      // after any `--prefix <dir>` / `run`), so `npm rebuild esbuild` parses
      // as the verb rebuild → skipped, and `npm run build` parses as build.
      for (const m of docs.matchAll(/`npm ([^`]+)`/g)) {
        const tail = m[1].trim()
        const prefix = tail.match(/(?:--prefix|--prefix=)\s*([\w./-]+)/)
        const dir = prefix ? prefix[1] : null
        const toks = tail.split(/\s+/).filter(Boolean)
        let i = 0
        if (toks[0] === '--prefix') i += 2
        if (toks[i] === 'run') i += 1
        const name = toks[i]
        if (!name || NPM_VERBS.has(name) || name.startsWith('-')) continue
        const scripts = scriptsFor(dir)
        if (!scripts || !(name in scripts)) {
          violations.push({ path: `${docsFile} mentions \`npm ${tail}\` but ${dir ? `${dir}/` : ''}package.json has no such script` })
        }
      }

      // Plain (unbackticked) npm refs → candidates when missing.
      for (const m of docs.matchAll(/\bnpm (?:--prefix\s+[\w./-]+\s+)?(?:(?:run)\s+)?([\w:-]+)/g)) {
        const script = m[1]
        if (NPM_VERBS.has(script)) continue
        const tail = m[0].replace(/^npm /, '')
        const prefix = tail.match(/(?:--prefix|--prefix=)\s*([\w./-]+)/)
        const dir = prefix ? prefix[1] : null
        const scripts = scriptsFor(dir)
        if (scripts && script in scripts) continue
        candidates.push({ path: `${docsFile}: plain npm reference "${script}" but ${dir ? `${dir}/` : ''}package.json has no such script — backtick if prose, fix if real` })
      }
    }
  }

  // 4. Accepted entries must cite a reviewable decision (an existing doc
  //    file) in their reason — 'accepted' means intentional, and intent must
  //    be verifiable or it becomes a free pass for any debt (s36 anti-gaming).
  for (const [key, entries] of Object.entries(rules.allowlists ?? {})) {
    for (const e of entries ?? []) {
      if (e.kind !== 'accepted') continue
      const cite = (e.reason ?? '').match(/(?:feature-parity\.md|AGENTS\.md|README\.md|docs\/[\w./-]+\.md)/)
      const id = e.path ?? e.name
      if (!cite) {
        violations.push({
          path: `accepted entry ${key}/${id}: reason must cite an existing doc (AGENTS.md, README.md, feature-parity.md, docs/*.md) — a decision must be reviewable`,
        })
      } else if (!existsSync(join(root, cite[0]))) {
        violations.push({
          path: `accepted entry ${key}/${id}: cited ${cite[0]} does not exist`,
        })
      }
    }
  }

  return { violations, candidates, info }
}

function frontmatter(text) {
  const out = {}
  // CRLF-tolerant: Windows checkouts (git autocrlf) carry \r\n.
  const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---/)
  if (!m) return out
  for (const line of m[1].split(/\r?\n/)) {
    const kv = line.match(/^([\w-]+):\s*(.*)$/)
    if (kv) out[kv[1]] = kv[2]
  }
  return out
}

function safeJson(text) {
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}