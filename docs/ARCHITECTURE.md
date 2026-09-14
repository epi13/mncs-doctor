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
  -> diagnosis   (host text diagnostics + MNCS scanner/version policy)
  -> health      (7 registered checks -> MNCS status/aggregation policy)
  -> planning    (MNCS edit/migration policy; dry-run renders diffs)
  -> transaction (fingerprint-validated, temp+rename, rollback)
  -> verification(MNCS composition, rescan, idempotence, external commands)
  -> report      (MNCS exit policy; host human/JSON rendering)
```

The live command path enters `DoctorMncsRuntime` before discovery. It verifies
and opens the checked-in ten-module family artifact once per process, retains
one session for all imported policy modules, validates every scalar return,
records entrypoints, and fails closed on initialization or call-contract
errors. `mncs-embed` is a normal runtime dependency pinned to the language
revision recorded in JSON reports.

## Modules and dependency direction

```text
main.rs (CLI launcher and host-effect orchestration)
  |
  +-- mncs_runtime (embedded sessions, ABI/provenance/fail-closed boundary)
  +-- discovery  (host acquisition + MNCS policy callback)
  +-- version    (no deps within crate)
  +-- diagnostics (discovery, version, fix::Applicability)
  +-- edits      (discovery [fingerprint], fix::Applicability, runtime policy)
  +-- fix        (diagnostics, discovery, edits, runtime policy)
  +-- migration  (diagnostics [scan_header], discovery, fix, runtime policy)
  +-- transaction(discovery [fingerprint], runtime policy)
  +-- health     (diagnostics, discovery, toolchain, version)
  +-- toolchain  (diagnostics [backend trait], discovery)
  +-- verify     (diagnostics, discovery, runtime policy)
  +-- report     (all of the above, host rendering)
```

Rules:

- `discovery` and `version` depend on nothing in-crate; everything else
  points at them, never the reverse.
- CLI handlers acquire argv, host facts, and effects, call runtime-backed
  library functions, render, and return the process code. Policy decisions
  do not silently fall back to the Rust oracle.
- Core logic is library-first (`mncs_doctor`): Forge/Ravel or future
  harnesses can consume planning and reporting without the CLI.
- Language knowledge enters through `LanguageBackend`, `MigrationRegistry`
  data, the mirrored profile table, and the pinned MNCS policy boundary.
  Upstream-owned gaps remain pressure entries (DOC-P-001…021).

## Key invariants

- Determinism: sorted traversal, sorted plans, sorted reports. Two runs
  over unchanged input produce byte-identical JSON modulo environment
  (toolchain paths/versions are reported, not hidden).
- Fail-closed mutation: fingerprint validation before *and* during commit,
  symlink refusal, unknown-edge refusal, review-gating. See `SAFETY.md`.
- Convergence: fix application iterates to a fixpoint with MNCS-backed
  oscillation detection (applied-provider tracking) and a 16-iteration budget.
- Provenance: every migration step records transition, kind, before/after
  fingerprints, and knowledge source; every report carries tool, schema,
  content versions, and MNCS source/artifact identities.

## Integration seams (stable by design)

- Forge: Doctor shells out to project build/test commands via
  `--verify-cmd`; deeper workspace orchestration remains owned by Forge and
  is not linked into Doctor. Doctor now discovers the optional `mncs-test`, `mncs-debug`, and
  Actions components and reports debugger protocol compatibility, but does
  not execute or interpret their semantic contracts. No Forge internals are
  linked.
- Ravel: migration records (before/after fingerprints + per-step
  provenance) are shaped to serve as future equivalence-check inputs.
  No Ravel internals are linked.
- Language service: the service has structured diagnostics, missing-import
  code actions, and identity-bound `FileEdit`/rename results, but no shared
  fix/edit contract with Doctor yet (DOC-P-001/002).
