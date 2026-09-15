# MNCS Doctor core

Real Doctor policy implemented in MNCS (profile 0.16), executed on the
`mncs-research-bytecode` backend via `mncs-embed`. This is not fixture
corpus: these modules decide version classification, health aggregation,
migration structure, edit conflicts, convergence, exit policy, and
verification composition. The Rust side keeps I/O mechanism (filesystem,
processes, rendering) plus the reference implementations the parity
suite compares against.

## Layout

`mncs/doctor/<area>.mncs` — one policy module per area. The
`mncs/doctor_family.mncs` freeze root imports all ten modules; its checked-in
`mncs/doctor/family.backend.json` is generated with the upstream compiler and
opened by the production runtime. See DOC-P-018 and
`docs/BACKEND-MATRIX-2026-09.md`. Semantic decisions that have a native
finite/record contract are consumed through the generated Rust binding
`src/generated/doctor_version.rs`; the remaining scalar seams below are
explicit transitional compatibility surfaces, not a second semantic ABI.

## Value contract (token_set pattern)

Codes cross the host boundary; records/enums live inside MNCS; human
strings render host-side.

| Domain | Encoding |
|--------|----------|
| versions | `(major: i64, minor: i64)` scalar pairs |
| classify | generated `VersionClass` finite value |
| compare | -1/0/1 (left relative to right) |
| severity | 0=info 1=warning 2=error |
| status | 0=pass 1=warning 2=fail 3=skipped |
| applicability | 0=safe 1=proven 2=review 3=manual |
| transition kind | 0=noop 1=metadata 2=source 3=unknown |
| plan verdict | 0=planned 1=noop 2=downgrade 3=off-line 4=blocked |
| stop rule | 0=continue 1=fixpoint 2=budget 3=oscillation |
| exit codes | 0/1/2/3 per `docs/EXIT-CODES.md` (4 is a host trap) |
| windows | fixed sequences (`[u64; 8]`, `[i64; 16]`) + live count; the value contract refuses wrong lengths/signedness fail-closed |

The registry (which profiles exist, which is current) is host-loaded
data: callers pass `cur_major/cur_minor` in, so no current-version
assumption is baked into these sources.

## Executing

Opened and called through the normal `mncs-embed` dependency (pinned rev;
see `Cargo.toml`, `src/mncs_runtime.rs`, and `tests/mncs_parity.rs`).
`DoctorMncsRuntime` verifies and opens the frozen family once per process,
then production `doctor`, `fix`, `migrate`, and `verify` reuse that session.
Regenerate the family with `scripts/freeze-doctor-family.sh`. Direct CLI
typecheck:

```sh
mncs source-study mncs/doctor/version.mncs
```

`CMP301` obligation notices (integer-overflow, exact-resource-cost)
are expected: arithmetic falls back to sound runtime checks and every
parity test executes through them green.

## Modules

- `version.mncs` — compare, classify against supplied current, support
  verdict, linear-chain span.
- `health.mncs` — combine, check_status, overall fold, severity rank.
- `migration.mncs` — span verdict, edge count, edge enumeration,
  unknown-edge scan, full plan verdict.
- `edits.mncs` — pairwise conflict, shape classification, kept-set scan.
- `fix.mncs` — eligibility gate, merge verdict, stop rule, seen-before.
- `report.mncs` — exit-code policy.
- `verify.mncs` — error-delta and three-channel composition verdicts.
- `transaction.mncs` — target presence, symlink, stale-base, and identical
  target validation policy.
- `discovery.mncs` — directory traversal and file classification policy.
- `scanner.mncs` — stateful 64-byte BOM/newline scanner ingress.
