# DECISIONS.md — Architectural & Product Decisions

Durable decisions that outlive any single session. Each entry records the
*why*, not just the *what*. Day-to-day scope guidance lives in `AGENTS.md`
(Scope Control); the milestone plan lives in `ROADMAP.md`.

## Integration Before Recreation

**Cyberdeck's value comes from orchestrating capabilities together, not from
independently reimplementing every capability available in the ecosystem.**

When cyberdeck needs something an existing open-source project, model,
runtime, framework, or tool already provides — and that implementation is
sufficiently capable, maintainable, and integrable — consume it through a
plugin, adapter, protocol, API, service, or runner seam. Do not turn the
dependency into a cyberdeck-owned subsystem.

The progression is **Need → Existing solution → Integration → Test →
Continue**, never Need → New subsystem → Scope growth → Core loop delayed.

A native cyberdeck implementation is appropriate only when the functionality
is core to cyberdeck (the measured-evidence loop: fit, bring-up, bench,
evaluation, recommendation), cannot reasonably be integrated, or integration
would be technically worse. A rebuild proposal must name the integration
boundary considered and the concrete reason it fails.

Established examples of this decision in practice:

- `opencode` stays an external binary behind a runner seam (`AgenticRunner` /
  `StatelessRunner`) — no forking, vendoring, or embedding a second agent.
- Harness config-routing is delegated to OSS (`AgentO`/`oneharness`), not rebuilt.
- All remote I/O shells out to system `curl`; no mandatory cloud/AI SDK dependency.
- Engine binaries resolve to system installs (`engine_bin`), not vendored builds.
- Ollama serves its own store; cyberdeck does not reimplement a model server.

Related: `ROADMAP.md` "Things Not To Build Yet (Explicit)", `FUTURE.md`
(the parking lot — interesting, not committed), `AGENTS.md` Scope Control.

## Runtime Adapters — Manifests Over Engine Matches

Runtimes are described by a `RuntimeManifest` (identity, formats,
architectures, capabilities, ports, status, `configuration` params, argv/env
templates) instead of an `Engine` enum arm plus a `match` at every launch site.
Builtins (llama.cpp / Uncensored / FreeToken / Ollama) are generated from the
existing `Engine` registry, so behavior is unchanged. A new backend is ONE JSON
file in `~/.local/share/cyberdeck/runtimes/*.json` — no core code, no
recompile. `deck-engines` resolves launch generically (`args_for` / `env_for` /
`unit_name_for`), so bring-up, test-port verification, unit install, and bench
treat an experimental runtime exactly like a builtin. A `Profile` binds a
custom runtime via `runtime_id` + structured `params` (values, not a baked CLI
string); placeholders are validated at load time so a typo names the file and
the bad key. Proven end-to-end by the mock-runtime test in
`deck-engines/tests/generic_runtime.rs`.

Deliberate ceiling: a custom runtime must expose the OpenAI-compatible HTTP
surface for health/bench (the manifest `protocol`); process-level plugins are
the upgrade path if a runtime needs a different lifecycle. This follows
"Integration Before Recreation" — the adapter is the boundary; cyberdeck does
not implement the runtime.
