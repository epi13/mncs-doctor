# DOC-P-021 — Pure backend values pass with unresolved obligations

Status: open

Category: compiler | backend | tooling

Severity: medium

Frequency: compiled-backend portability evidence

## Doctor workload

Doctor needs executable value evidence for pure policy modules on every
supported backend before changing its production backend or claiming
portable artifacts.

## Reproducer

Using `mncs experiment run` at `mncs-language` `a0255f8`, each of the ten
Doctor policy sources was run against its checked-in value corpus on:

```text
mncs-research-bytecode
mncs-portable-wasm-mvp
mncs-c11
mncs-llvm-ir
mncs-cranelift
```

All expected values were returned for all fifty module/backend runs. Six
modules nevertheless reported compiler `status: UNKNOWN` with
`unresolved_reasons: ["compilation retained required unresolved obligations"]`:
`version`, `health`, `migration`, `edits`, `fix`, and `scanner`. The returned
values were correct, but the evidence status cannot be recorded as an
unqualified backend PASS.

## Observed impact

Doctor can safely run pure policy on the research bytecode backend, where
value execution is proven. Compiled-backend adoption would either reject
these artifacts or require a documented policy for value-correct but
obligation-unknown results. Treating `UNKNOWN` as `PASS` would weaken the
fail-closed portability claim.

## Workaround

Production pins `mncs-research-bytecode`. The matrix is retained as returned
value evidence, with `UNKNOWN` preserved as the compiler result rather than
hidden by the harness. The ten corpora under `fixtures/backend/` and
`docs/BACKEND-MATRIX-2026-09.md` reproduce the run.

## Removal condition

The compiler/backend pipeline either discharges the obligations for these
bounded pure modules or emits a stable distinction between executable value
evidence and unresolved proof obligations. Re-run the complete matrix and
require identical values plus an explicit non-UNKNOWN evidence status.

## Evidence

- `fixtures/backend/*.json` — value-level source corpora.
- `docs/BACKEND-MATRIX-2026-09.md` — fifty executed matrix cases and
  classification.
- `mncs-language` `experiment run` output at executable tree `a0255f8`;
  main later advanced to badge-only `e6d9238`.
