# DOC-P-018 — `from_source` refuses `use` imports (no embed freeze path)

Status: fixed-upstream

Category: tooling (embedding/packaging)

Severity: low

Frequency: structural (every multi-module MNCS program)

## Doctor workload

`mncs/doctor/*.mncs` is ten modules with shared scalar conventions.
The natural shape is one module family with imports; tiny helpers are
currently duplicated per file instead.

## Current behavior (verified 2026-09-12)

`Artifact::from_source` refuses sources with `use` imports
(`crates/mncs-embed/src/lib.rs`: "library resolution belongs to the
build step that froze them"). The compiler CLI now provides that build
step: with `MNCS_LIBRARY_PATH`, `mncs compile SOURCE --emit backend
--target mncs-research-bytecode --output-dir DIR` resolves imports and
emits `backend.json`; `mncs experiment execute` then runs that frozen
artifact. A probe importing `doctor.version.v1`, `doctor.health.v1`, and
`doctor.report.v1` compiled into one artifact and all three calls returned
their expected values.

`Artifact::from_source` itself remains intentionally source-only. Doctor now
checks in `mncs/doctor_family.mncs` plus the generated
`mncs/doctor/family.backend.json`; the production runtime verifies and opens
that one artifact rather than compiling ten sources at startup. The checked-in
`fixtures/backend/family-corpus.json` all-ten wrapper corpus returned all
expected values at current main; the artifact is 5,011,518 bytes and retains
29 compiler unresolved-obligation notices, which are tracked separately by
DOC-P-021.

## Resolution

The production runtime uses the frozen family. The regeneration command is
`scripts/freeze-doctor-family.sh`; it supplies the Doctor source root and the
upstream stdlib root to the compiler and replaces the checked-in artifact.

## Closure condition

The upstream freeze step is present and executable, and Doctor has adopted it.
Keep the artifact regeneration script, raw digest check, canonical identity
validation, and language-revision pin in sync when the frozen artifact format
changes.

## Evidence

`mncs compile` + `mncs experiment execute` imported-family probe at
`mncs-language` executable tree `a0255f8`; the later main badge refresh
`e6d9238` did not change the compiler or embed implementation. `mncs bundle
generate/verify` also produced the content-addressed stdlib closure used by
the compiler.
