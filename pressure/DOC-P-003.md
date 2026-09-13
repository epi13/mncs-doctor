# DOC-P-003 — No published version-transition rules

Status: workaround

Category: language | tooling

Severity: medium

Frequency: pervasive (every `migrate` invocation)

## Doctor workload

`migrate --to latest` over a 0.8 repository must compose bounded adjacent
transitions (0.8 → 0.9 → … → 0.16) with per-edge provenance.

## Minimal reproducer

`mncs-doctor migrate --to latest --plan` on `fixtures/repos/outdated`
reports every edge as `unknown` with provenance
`doctor-seed: unrecorded upstream`, and `--apply` refuses (exit 4).

## Current behavior

- `default_registry()` records all 15 adjacent edges as `Unknown`.
- Planning works (shortest forward path, no-op handling, downgrade
  refusal); apply is fail-closed across unknown edges.
- The only doctor-owned mutation is the mechanical header bump (REVIEW),
  plus a JSON `--registry` extension point and synthetic 9.x fixtures
  proving the engine (`fixtures/migration/`, 5 integration tests).

## Desired behavior

Upstream publishes per-edge transition knowledge (source transforms with
semantic guarantees, metadata steps, no-op markers, review-required
flags) as versioned data Doctor can consume through `--registry` without
code changes.

## Likely ownership

language (transition authorship); Doctor (orchestration, already built).

## Impact

- Functionality: production migration is plan-only today — honest, but
  the flagship workflow awaits upstream data.
- Safety: the fail-closed default is the correct posture, not a gap to
  work around by guessing transforms.
