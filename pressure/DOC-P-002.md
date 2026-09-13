# DOC-P-002 — No shared structured fix/edit contract upstream

Status: workaround

Category: language | tooling

Severity: high

Frequency: common

## Doctor workload

`fix --dry-run` must show exactly what `fix` would apply; an LSP code
action and `mncs-doctor fix` should invoke the same repair semantics.

## Minimal reproducer

Upstream search: `autofix|auto_fix|apply_fix|CodeAction|remediation`
across `mncs-language` yields nothing; the language service offers exactly
one fix family (MNE131 missing-import `use` insertion) plus compute-only
rename. There is no upstream edit representation (spans, conflict rules,
ordering, stale-base handling).

## Current behavior

Doctor defines its own `TextEdit`/`EditSet` (span replacement, overlap
conflicts, deterministic ordering, SHA-256 stale-base refusal) and a
`Diagnostic` envelope shaped as a *proposal* for the shared contract
(code, severity, span, message, explanation, applicability, version
metadata, migration transition). Built-in providers cover SAFE hygiene
only (whitespace, final newline, CRLF, BOM).

## Why the workaround is insufficient

- Doctor-side fixes cannot address semantic diagnostics (MNE*) — only the
  language side knows the authoritative basis for those.
- Two edit representations (service-local `FileEdit`, doctor `EditSet`)
  will drift unless unified upstream.

## Desired behavior

Upstream adopts (or supersedes, with migration) a shared diagnostic/fix
envelope: codes, spans, applicability levels, edit sets with conflict and
staleness semantics, and per-code authoritative fix providers. The service
and Doctor both become consumers.

## Likely ownership

language (contract + per-code providers); service/Doctor adopt.

## Impact

- Safety: semantic autofix is correctly absent rather than guessed.
- Implementation complexity: Doctor's engine is ready; providers await
  upstream knowledge.
