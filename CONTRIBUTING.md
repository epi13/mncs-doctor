# Contributing

## Boundary first

The `mncs-language` / `mncs-doctor` split is the project's core invariant:
language knowledge (syntax, parsing, semantics, diagnostics, fixes,
canonicalization, transitions, version metadata) belongs upstream. If a
feature needs it and upstream lacks it:

1. file a pressure entry (`pressure/DOC-P-XXX.md`, see `pressure/README.md`),
2. implement the narrowest stopgap inside the existing seam
   (`LanguageBackend`, `MigrationRegistry`, mirrored tables),
3. document the removal condition in the module docs.

Never duplicate the parser, resolver, or transition rules here.

## Workflow

- Rust 1.85+, `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`.
- Keep CLI handlers thin; core logic goes in the library with unit tests.
- CLI-visible behavior (commands, flags, exit codes, JSON shape) needs an
  integration test in `tests/` plus a fixture under `fixtures/` — staged
  copies only, never mutate committed fixtures.
- Additive JSON changes only (new optional fields); bump
  `REPORT_SCHEMA_VERSION` on breaking changes.
- One logical commit per change; branch `feat/<topic>`; merge to `main`
  after green CI.

## Safety-critical code

`src/transaction.rs`, `src/fix.rs`, `src/migration.rs::apply_plan`:
changes need tests covering stale bases, conflicts, oscillation,
idempotence, and permission preservation. When in doubt, refuse.
