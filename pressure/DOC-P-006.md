# DOC-P-006 — Study-JSON contract is useful but unversioned and file-local

Status: workaround

Category: tooling

Severity: low

Frequency: common (every `RustCliBackend` invocation)

## Doctor workload

`RustCliBackend` runs `mncs source-study <file>` per file and needs
machine-readable diagnostics.

## Discovery (2026-09-13)

`source-study` already emits a JSON artifact on stdout with a
`diagnostics` array (`{code, stage, severity, message,
span{start, end, line, column}, expected, found}`) — verified against the
live CLI (MNE131 with byte span observed). Doctor now consumes it with a
version-tolerant mapping (`toolchain.rs::map_study`), falling back to a
coarse `DOC201` only when the output is unparseable.

Remaining gaps (why this stays pressure, not closed):

1. The schema is unversioned and undocumented — Doctor's mapping can only
   be tolerant, never certain.
2. Invocation is file-local: cross-module facts (imports, workspace
   resolution) cannot be supplied, so single-file runs report resolution
   failures (MNE131/MNE173-class) that project-aware analysis would clear.
3. Exit-code semantics for "studied with findings" vs "tool failed" are
   inferred, not specified.

## Desired behavior

Version the study envelope (or fold it into the DOC-P-001 programmatic
API) and accept workspace context (library path, module roots) so
project-aware backends stop paying per-file process costs for known-
unresolvable single-file runs.

## Likely ownership

tooling (schema version + workspace-aware verb); Doctor adopts.
