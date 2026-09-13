# DOC-P-017 — No source-level multi-backend value-I/O harness

Status: open

Category: tooling (compiler/backend verification)

Severity: low (execution is correct on the sanctioned backend)

Frequency: structural (portability claims for `mncs/doctor/`)

## Doctor workload

The MNCS core (pure policy, no effects) should eventually be portable
beyond the research interpreter — or the boundary of that claim should
be explicit per backend.

## Current behavior (verified 2026-09-13)

`mncs-embed` executes on `mncs-research-bytecode`; compiled backends
(C11/WASM/LLVM/Cranelift) refuse *effects*, and pure-code portability
is untestable at the source level: `conformance` is predicate-based
(generated cases over contracts), not value-I/O over entrypoints, and
`execute`/`compile` consume manifest JSON, not `.mncs` sources with
arguments. Doctor therefore claims bytecode execution only.

## Workaround

Pin execution to the research backend in tests; record per-module
backend status as `bytecode: verified; compiled: untested (no harness)`.

## Removal condition

A value-level multi-backend runner for `.mncs` sources (entrypoint +
arguments → observed values per backend); then run the parity suite
across the matrix and file per-backend divergences as C-class pressure.

## Evidence

`docs/RUST-BOUNDARY-AUDIT.md` (backend finding); `tests/mncs_parity.rs`
(session construction pins `mncs-research-bytecode`).
