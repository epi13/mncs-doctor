# MNCS Doctor core

Real Doctor policy implemented in MNCS (profile 0.16), executed on the
`mncs-research-bytecode` backend via `mncs-embed`. This is not fixture
corpus: these modules decide version classification, health aggregation,
migration structure, edit conflicts, convergence, exit policy, and
verification composition. The Rust side keeps I/O mechanism (filesystem,
processes, rendering) plus the reference implementations the parity
suite compares against.

## Layout

`mncs/doctor/<area>.mncs` — one self-contained module per area (no
`use` imports: `Artifact::from_source` executes frozen self-contained
sources; see DOC-P-018). Shared scalar contract below; tiny helpers are
duplicated per file deliberately until a multi-module freeze step lands.

## Value contract (token_set pattern)

Codes cross the host boundary; records/enums live inside MNCS; human
strings render host-side.

| Domain | Encoding |
|--------|----------|
| versions | `(major: i64, minor: i64)` scalar pairs |
| classify | 0=current 1=sealed 2=unsupported 3=unknown |
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

Compiled and called through `mncs-embed` (pinned rev, see
`Cargo.toml` dev-dependencies and `tests/mncs_parity.rs` for the
calling convention). Direct CLI typecheck:

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
