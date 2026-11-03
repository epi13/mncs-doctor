# DOC-P-022 — No native remediation-classification policy module

Status: fixed-upstream (2026-11-03)

Category: tooling | compiler

Severity: low

Frequency: every `remediate` run

## Doctor workload

`remediate` classifies repair applicability, convergence stops, and residual
diagnostic severity. This pure policy belongs in MNCS beside the existing
fix/health/report modules, with the shared classification law owned by
Commons.

## Minimal reproducer

```rust
// Desired natural expression (does not exist):
let class = policy.remediate_classify(applicability, stop, severity, budget)?;
```

## Current behavior

The original Rust class and severity matches are retired. Doctor now calls
the imported Commons policy through the embedded family session for repair
classes, stop dispositions, residual escalation, degraded-file counts, and
blocker counts. Rust retains Doctor-specific code/action strings, report
projection, and the bounded work loop. The current `mncs-embed` revision
executes the 0.18 cross-root import, and the generated binding uses the
family artifact's aggregate interface identity.

## Removal condition

Resolved by consuming the existing Commons classifier through a thin
Doctor MNCS adapter, freezing the module into the production family, and
retiring the matching Rust classification paths.

## 2026-10-01 re-evaluation (historical status: blocked)

The policy half is done: the classification law now exists natively in
Commons (`mncs.commons.family.remediation.v1`: `classify_repair`,
`classify_stop`, `classify_residual`), is proven executable by
`tests/test_native_remediation_contract.py`, and doctor's envelope
already speaks its vocabulary (`mncs.remediation/1`). What remains is
consumption, and it is blocked by a precise runtime wall, proven this
run:

- Importing the 0.18 Commons module into `mncs/doctor_family.mncs`
  compiles into a valid artifact under the current compiler, and the
  backend corpus gate passed 13/13 after mechanical integer-to-finite
  expectation migration.
- But the pinned production runtime (mncs-embed @25525f0, via the Cargo
  git pin in `Cargo.lock`) rejects EVERY typed call against any
  artifact containing the cross-root import with
  `ExecutionStatus::InvalidRequest` — including pre-existing entrypoints
  such as `stop_rule`, so this is not a new-code defect. Old bindings
  against the new artifact fail with an interface-identity mismatch.
- All family-import changes were therefore reverted; doctor stays green
  on its frozen 0.16 family with the tested Rust classification paths.

The removal condition at that time was to re-pin mncs-embed to a compiler revision
whose runtime executes 0.18 cross-root imports (blocked on the active
compiler agent's finite/enum work settling — do not force), then
re-freeze with the Commons import plus thin wrappers, regenerate
bindings, extend parity tests, and delete the Rust paths.

Alternatives considered and rejected: per-file `mncs call` subprocesses
from doctor's hot path (~100ms each plus a toolchain runtime dependency
in a batch repair tool), and vendor-duplicating the law into a 0.16
doctor module (a parallel standard, forbidden by Commons ownership of
`mncs.remediation/1`).


## 2026-11-03 resolution

Doctor added `mncs/doctor/remediate.mncs` as an adapter to the existing
`mncs.commons.family.remediation.v1` law, froze it into the production family,
and declared the Commons dependency in its project manifest. Runtime
integration tests compare native results against independent vectors for
granted/blocked repairs, all stop classes (including unknown codes), and
residual error/warning/info behavior. `tests/remediate.rs` and
`tests/health_epoch.rs` cover the real command and cached re-emission.

The previous block came from a stale `mncs-embed` pin and a generated Rust
binding decoder that rejected the pair-array record shape returned by typed
native calls. Language revision `2317e629` repairs that decoder; Doctor pins
it and checks the generated root binding against the frozen aggregate interface
identity. Evidence is recorded in
`evidence/native-remediation-classification-20261103.json`.
