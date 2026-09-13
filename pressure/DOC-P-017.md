# DOC-P-017 — No source-level multi-backend value-I/O harness

Status: fixed-upstream

Category: tooling (compiler/backend verification)

Severity: low (execution is correct on the sanctioned backend)

Frequency: structural (portability claims for `mncs/doctor/`)

## Doctor workload

The MNCS core (pure policy, no effects) should eventually be portable
beyond the research interpreter — or the boundary of that claim should
be explicit per backend.

## Current behavior (verified 2026-09-12)

The upstream `experiment run` command now supplies the missing source-level
value harness: it takes an `.mncs` source, a corpus of entrypoint requests,
and a backend, then records returned values and expectation matches. Doctor
ran all ten pure policy modules against their checked-in corpora on five
backends (fifty value cases total). Every expected value matched.

Six modules (`version`, `health`, `migration`, `edits`, `fix`, `scanner`)
still carry a separate compiler `UNKNOWN` status because unresolved
obligations remain; that is recorded as DOC-P-021, not hidden here.

## Resolution

Doctor keeps production on the research bytecode backend until DOC-P-021 is
resolved, but portability is now executable and auditable. The ten corpora
under `fixtures/backend/` and the matrix transcript in
`docs/BACKEND-MATRIX-2026-09.md` are the regression evidence.

## Closure condition

This record is closed by the upstream value runner. Remaining backend
evidence quality is tracked by DOC-P-021.

## Evidence

`docs/BACKEND-MATRIX-2026-09.md`; `fixtures/backend/`; and
`tests/mncs_parity.rs` (the production embed session still pins
`mncs-research-bytecode`). The executable language tree was `a0255f8`; the
later `mncs-language` main badge refresh (`e6d9238`) did not change it.
