# Architecture

`mncs-doctor` repairs MNCS *projects*; `mncs-language` repairs MNCS
*source*. The boundary is load-bearing: Doctor owns orchestration over
read-only language artifacts and never reimplements syntax, parsing,
semantics, canonicalization, or transition knowledge.

## Data flow

```text
repository
  -> discovery   (root, walk, exclusions, deterministic inventory)
  -> inventory   (bytes, fingerprints, newline/BOM/mode facts)
  -> diagnosis   (LanguageBackend: scanner today, language APIs tomorrow)
  -> health      (7 registered checks -> structured findings)
  -> planning    (fix plans | migration paths; dry-run renders diffs)
  -> transaction (fingerprint-validated, temp+rename, rollback)
  -> verification(rescan, idempotence, optional external commands)
  -> report      (human summary | versioned JSON, exit code)
```

## Modules and dependency direction

```text
main.rs (CLI handlers: thin; no repair logic)
  |
  +-- discovery  (no deps within crate)
  +-- version    (no deps within crate)
  +-- diagnostics (discovery, version, fix::Applicability)
  +-- edits      (discovery [fingerprint], fix::Applicability)
  +-- fix        (diagnostics, discovery, edits)
  +-- migration  (diagnostics [scan_header], discovery, fix)
  +-- transaction(discovery [fingerprint])
  +-- health     (diagnostics, discovery, toolchain, version)
  +-- toolchain  (diagnostics [backend trait], discovery)
  +-- verify     (diagnostics, discovery)
  +-- report     (all of the above, render only)
```

Rules:

- `discovery` and `version` depend on nothing in-crate; everything else
  points at them, never the reverse.
- CLI handlers orchestrate: parse args, call library functions, render.
  No fix/migration/transaction logic lives in `main.rs`.
- Core logic is library-first (`mncs_doctor`): Forge/Ravel or future
  harnesses can consume planning and reporting without the CLI.
- Language knowledge enters only through `LanguageBackend`,
  `MigrationRegistry` data, and the mirrored profile table — all three
  documented as upstream-owned with pressure entries (DOC-P-001…007).

## Key invariants

- Determinism: sorted traversal, sorted plans, sorted reports. Two runs
  over unchanged input produce byte-identical JSON modulo environment
  (toolchain paths/versions are reported, not hidden).
- Fail-closed mutation: fingerprint validation before *and* during commit,
  symlink refusal, unknown-edge refusal, review-gating. See `SAFETY.md`.
- Convergence: fix application iterates to a fixpoint with oscillation
  detection (applied-provider tracking) and a 16-iteration budget.
- Provenance: every migration step records transition, kind, before/after
  fingerprints, and knowledge source; every report carries tool, schema,
  and content versions.

## Integration seams (stable by design)

- Forge: Doctor shells out to project build/test commands via
  `--verify-cmd`; deeper workspace orchestration awaits a stable Forge
  API. No Forge internals are linked.
- Ravel: migration records (before/after fingerprints + per-step
  provenance) are shaped to serve as future equivalence-check inputs.
  No Ravel internals are linked.
- Language service: the `Diagnostic` envelope is a proposal for the
  shared contract so LSP code actions and `fix` can share providers
  (today only the shape is shared; see DOC-P-002).
