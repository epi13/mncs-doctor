# DOC-P-018 — `from_source` refuses `use` imports (no embed freeze path)

Status: open

Category: tooling (embedding/packaging)

Severity: low

Frequency: structural (every multi-module MNCS program)

## Doctor workload

`mncs/doctor/*.mncs` is seven modules with shared scalar conventions.
The natural shape is one module family with imports; tiny helpers are
currently duplicated per file instead.

## Current behavior (verified 2026-09-13)

`Artifact::from_source` refuses sources with `use` imports
(`crates/mncs-embed/src/lib.rs`: "library resolution belongs to the
build step that froze them"), but no documented source→frozen-artifact
freeze step for multi-module programs was found on the CLI (the `bundle`
verb handles execution bundles, a different artifact). So embeddable
programs must be self-contained single sources today.

## Workaround

One self-contained file per policy area; duplication is small
(ordering codes, window idioms) and parity-tested per file.

## Removal condition

A supported freeze step (multi-module source + library path → frozen
artifact with imports resolved) lands; then factor `mncs/doctor/` into
a proper module family with shared helpers and keep the parity suite
green across the refactor.

## Evidence

`mncs/doctor/*.mncs` headers (no `use` lines by construction);
`parse` of the embed refusal message in `from_source_with_seeds`.
