# mncs-doctor

MNCS repository diagnostics, automated repair, and version-aware source
migration tooling.

> `mncs-language` knows how to repair MNCS source.
> `mncs-doctor` knows how to repair an MNCS project.

## What it is

A developer tool that inspects an MNCS workspace and reports health,
applies safe repairs transactionally, and plans version migrations as
composable adjacent transitions — with dry-run everywhere, structured
JSON reports, and fail-closed mutation semantics.

## What it is not

- Not a parser, typechecker, or migration-rule author: language knowledge
  lives in `mncs-language` (see [Boundary](#boundary)). Doctor orchestrates.
- Not a formatter with opinions beyond normalization; not a VCS.
- Not production-complete for historical migration: transition rules are
  unrecorded upstream, so `migrate --apply` on real profiles refuses until
  they land (see `docs/MIGRATION.md`). Planning works today.

## Maturity: experimental but honest (0.1.0)

Working: discovery, 7 health checks, hygiene diagnostics with spans, safe
fix engine with convergence loop, transactional writes with rollback,
migration planning/registry/provenance, human + JSON reports, 88 tests.
Blocked on upstream data: semantic diagnostics, per-code fix providers,
real transition rules. Every gap is a pressure entry, not a mock.

## MNCS core

Doctor policy increasingly lives in MNCS, not just fixtures:
`mncs/doctor/` holds seven executed modules (version, health, migration,
edit, fix, report, verify policy) running on the research bytecode
backend via `mncs-embed`. Scalar codes cross the host boundary;
records/enums live inside MNCS; strings render host-side. `tests/
mncs_parity.rs` proves the MNCS core agrees with the Rust reference on
every probed input, and pins fail-closed transport (length/signedness/
identity mismatches refuse as `invalid_request`). See `mncs/README.md`
and `docs/RUST-BOUNDARY-AUDIT.md` for the boundary: what moved, what
stays host-side, and the exact removal condition per subsystem.

## Commands

```text
mncs-doctor doctor [--check] [--explain] [--json]
mncs-doctor fix [--dry-run] [--safe-only] [--proven] [--verify-cmd "<cmd>"]
mncs-doctor migrate --to <version|latest> [--plan] [--dry-run] [--apply]
                   [--allow-review] [--registry <file>] [--verify-cmd "<cmd>"]
mncs-doctor verify [--verify-cmd=<cmd>]
```

Global: `--root <dir>`, `--json`, `--explain`, `--quiet`, `--verbose`,
`--with-language-backend` (opt-in Rust CLI semantic probe),
`--no-color`. Exit codes in `docs/EXIT-CODES.md`.

Example:

```text
$ mncs-doctor doctor --explain
MNCS repository health

Source:
  412 files checked
  0 errors
  3 warnings
  29 informational (sealed profiles)

Checks:
  source-parse-health      pass
  version-drift            warning
  ...
```

## Boundary

`mncs-language` owns: syntax, parsing, semantics, diagnostics, structured
fixes, canonicalization, transition knowledge, AST/CST transforms, version
metadata. `mncs-doctor` owns: discovery, inventory, health analysis,
migration planning/orchestration, fix application, dry-run/reporting,
toolchain checks, verification, transactions, rollback, review workflows,
machine-readable reports. Doctor consumes language artifacts through the
`LanguageBackend` trait and `MigrationRegistry` data; where the toolchain
cannot supply them yet, narrow stopgaps are documented in `pressure/` with
removal conditions.

## Relations

- **Language service**: the `Diagnostic` envelope is a proposal for the
  shared contract so LSP code actions and `fix` share providers.
- **Forge**: project build/test via `--verify-cmd`; deeper orchestration
  awaits a stable API (nothing linked).
- **Ravel**: migration records carry fingerprints + provenance shaped for
  future equivalence evidence (nothing linked).

## doctor vs fix vs migrate

- `doctor`: inspect only, never mutates. Health checks + explanations.
- `fix`: safe repairs within the current version (whitespace, newlines,
  BOM today). Dry-run default-off flag; convergence + verification.
- `migrate`: cross-version movement. Plan is always safe to run; apply is
  fail-closed across unknown/review edges.

## Safety

Commit precious trees to git first. Doctor validates fingerprints before
and during writes, writes via temp+rename, preserves permissions, refuses
symlinks, and rolls back partial failures. Details: `docs/SAFETY.md`.

## Development

```sh
cargo build
cargo test          # 56 unit + 18 CLI integration tests
cargo fmt --check
cargo clippy -- -D warnings
```

Layout: `src/` (library modules + thin CLI), `tests/` (CLI matrix),
`fixtures/` (repo shapes + 9.x migration corpus), `pressure/` (language
gaps), `docs/` (architecture, safety, migration, exit codes).
See `CONTRIBUTING.md`.
