# DOC-P-001 — No stable programmatic diagnostics API for tooling

Status: workaround

Category: tooling

Severity: high

Frequency: pervasive

## Doctor workload

Every Doctor command needs per-file diagnostics with source spans:
`doctor` aggregates them, `fix` re-diagnoses to a fixpoint, `migrate`
verifies before/after each transition, `verify` compares error counts.

## Minimal reproducer

```rust
// Desired natural expression (does not exist):
let report = mncs_language::diagnose_file(&source_file)?; // spans + codes
```

There is no published crate (`mncs-syntax`/`mncs-model` are not on a
registry; depending on them means a git/path dependency on a moving
workspace), and the CLIs operate on JSON manifests, not on raw `.mncs`
source trees.

## Current behavior

Doctor ships two backends behind the `LanguageBackend` trait:

1. `ScannerBackend` — a shallow header/module/delimiter scan. It mirrors
   only the *documented* header rule and performs no semantic analysis.
2. `RustCliBackend` (opt-in `--with-language-backend`) — shells out to the
   Rust CLI's `source-study` verb and maps nonzero exit to one `DOC201`
   diagnostic. CLI output is deliberately *not* parsed (no output contract
   exists; see DOC-P-006).

## Why the workaround is insufficient

- The scanner cannot see semantic errors (unresolved names, type errors),
  so `fix` convergence and `verify` are hygiene-deep only.
- The subprocess backend is coarse (one diagnostic per file), slow per
  file, and has no timeout/cancellation.
- Header semantics are now mirrored in two places (risk of drift; see
  DOC-P-005).

## Desired behavior

A versioned programmatic entry point (crate or stable CLI JSON verb) that
takes source text plus a profile and returns structured diagnostics with
byte spans and codes — the same artifact the language service consumes.

## Likely ownership

language (API surface) + tooling (packaging).

## Impact

- Correctness: semantic issues are invisible to Doctor today.
- Safety: convergence claims cover hygiene only; documented as such.
- Implementation complexity: every new deep check needs upstream work
  first; Doctor cannot prototype it locally without duplicating the parser
  (which the architecture forbids).

## Evidence

- `src/diagnostics.rs` (`LanguageBackend`, `ScannerBackend`); the bounded
  BOM/newline ingress now runs through `doctor.scanner.v1`, while detailed
  text diagnostics remain host-side until a structured upstream API exists.
- `src/toolchain.rs` (`RustCliBackend`, fail-closed output mapping)
- `tests/doctor.rs` and `tests/mncs_parity.rs` (live production provenance
  plus transport/parity coverage)
