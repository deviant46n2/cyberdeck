// integrity-artifacts.mjs — artifact-hygiene section for cyberdeck.
//
// Replaces ModCanvas's asset-bundle section (that one guarded game-derived
// image bytes; cyberdeck guards two different leaks):
//   1. Model blobs / partial downloads must never land in the repo tree —
//      .gguf/.safetensors/etc live in ~/models, NOT in git. A committed
//      .part is a paused transfer checked in by mistake; a committed model is
//      a multi-GB tar-pit that makes the repo unusable for everyone.
//   2. Secrets must never be tracked: .env files, private keys, and
//      credential files. HF tokens flow through env vars, never the tree.
//
// Data-driven via rules.artifactDirs + rules.artifactPatterns + allowlist.

import { execFileSync } from 'node:child_process'
import { readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

// Matched against a file's basename (lowercased). `.part` is a stayed transfer
// (dl.ts STOP semantics); archives and weights are the blobs that break git.
const BLOB_PATTERN = /\.(gguf|safetensors|part|ckpt|pt|pth|onnx|bin|model)$/i
// Any archive over the size gate is treated as an accidental artifact too —
// a 1.3 GB source.conf.zip is a download, not a dependency.
const ARCHIVE_PATTERN = /\.(zip|tar|gz|7z|rar|iso)$/i
const BIG_BLOB_BYTES = 64 * 1024 * 1024 // 64 MiB

// Tracked-file names that must never appear in `git ls-files` (basename match,
// allowing `.env.example` style docs but not `.env` / `.env.local` / `.env.prod`).
const SECRET_PATTERN = /\.env(\.\w+)?$|\.(pem|p12|pfx|key|p8)$|^id_rsa|\.secret$|^credentials?\b/i

export function checkArtifactHygiene(rules, root) {
  const violations = []
  const parked = []
  const accepted = []
  const allow = rules.allowlists['artifact-hygiene'] ?? []

  // 1. Tree scan for model blobs / partials / oversized archives.
  for (const dir of rules.artifactDirs) {
    const abs = join(root, dir)
    if (!existsSyncSafe(abs)) continue
    const stack = [abs]
    while (stack.length) {
      const current = stack.pop()
      let entries
      try {
        entries = readdirSync(current, { withFileTypes: true })
      } catch {
        continue
      }
      for (const e of entries) {
        const p = join(current, e.name)
        if (e.isDirectory()) {
          stack.push(p)
          continue
        }
        const rel = relativeSafe(root, p)
        const base = e.name
        if (BLOB_PATTERN.test(base) || ARCHIVE_PATTERN.test(base)) {
          let isBig = false
          try {
            isBig = ARCHIVE_PATTERN.test(base) && statSync(p).size > BIG_BLOB_BYTES
          } catch {
            /* unreadable — still a violation below */
          }
          if (blobLike(base) || isBig) {
            const entry = allow.find((a) => a.path === rel)
            if (entry) {
              if (entry.kind === 'accepted') accepted.push({ path: rel, reason: entry.reason })
              else parked.push({ path: rel, reason: entry.reason })
            } else {
              violations.push({ path: rel, message: `${base} must not be committed to the repo — model files live in ~/models` })
            }
          }
        }
      }
    }
  }

  // 2. Tracked secrets: git is the authority (`git ls-files`) so gitignored
  //    files never false-flag, and a .env added to the index is caught.
  try {
    const tracked = execFileSync('git', ['ls-files'], { cwd: root, encoding: 'utf8' })
    for (const line of tracked.split('\n')) {
      const file = line.replace(/\\/g, '/')
      if (!file) continue
      const base = file.split('/').pop() ?? file
      if (!SECRET_PATTERN.test(base)) continue
      const entry = allow.find((a) => a.path === file)
      if (entry) {
        if (entry.kind === 'accepted') accepted.push({ path: file, reason: entry.reason })
        else parked.push({ path: file, reason: entry.reason })
      } else {
        violations.push({ path: file, message: 'tracked secret — rotate and remove from the index' })
      }
    }
  } catch {
    violations.push({ message: 'git ls-files failed — cannot verify no tracked secrets' })
  }

  return { violations, parked, accepted }
}

function blobLike(base) {
  return BLOB_PATTERN.test(base)
}

function existsSyncSafe(p) {
  try {
    const s = statSync(p)
    return s.isDirectory() || s.isFile()
  } catch {
    return false
  }
}

function relativeSafe(root, p) {
  const rel = p.startsWith(root + '/') ? p.slice(root.length + 1) : p
  return rel.split('\\').join('/')
}