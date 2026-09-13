# Migration model

Migrations compose bounded adjacent transitions (`0.15 -> 0.16`), never
monolithic ancient-to-latest rewrites.

## Edge kinds

| Kind | Meaning on apply |
|------|------------------|
| `noop` | No source changes; the header stamp still advances |
| `metadata` | Mechanical header bump (doctor-owned, `review`-gated) |
| `source` | Recorded transforms, then the header bump |
| `unknown` | No recorded knowledge: planning reports it, apply refuses it |

## Planning

`plan(from, to)` finds the shortest forward path over adjacent edges.
Same version → empty no-op plan. Downgrades → `NoPath` (no
down-migration knowledge exists). Versions outside the registry graph →
`NoPath` with the reason.

The path is evaluated by the production `doctor.migration.v1` policy module
after the host registry supplies transition facts. The host still owns the
transition data and byte-level transform mechanism; an MNCS `blocked` verdict
is fail-closed and must agree with the registry plan before application.

## Current coverage (honest)

Upstream publishes no source-level transition rules, so the production
registry records all 15 adjacent edges as `unknown`:

- `migrate --plan` / `--dry-run`: fully functional (paths, provenance,
  per-file inspect).
- `migrate --apply`: fail-closed on real profiles (exit 4, names the
  blocking edge). Pass `--allow-review` only crosses `review` edges —
  there are no production `review` edges yet, so `--apply` on real
  profiles always refuses today.
- `--registry <file>`: loads additional transitions (JSON
  `RegistryFile`) — the seam future upstream data will use without code
  changes. `fixtures/migration/registry-9x.json` demonstrates the format.
- Synthetic 9.x `source`/`noop` transitions prove the engine end to end
  (apply → verify → re-plan is no-op), clearly marked `fixture/*`
  provenance that can never collide with real profiles.

## Provenance

Every applied step records transition, kind, before/after SHA-256,
replacement count, and knowledge source (`AppliedStep`); per-file records
(`MigrationRecord`) ship in the JSON report for audit or future
Ravel equivalence checks.
