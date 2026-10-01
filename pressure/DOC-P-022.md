# DOC-P-022 — No native remediation-classification policy module

Status: open

Category: tooling | compiler

Severity: low

Frequency: every `remediate` run

## Doctor workload

`remediate` classifies each outcome as safe-automatic, bounded
reconciliation, or escalation (`src/remediate.rs`). The classification is a
pure policy over finite inputs (applicability level, stop reason, severity,
budget state) and belongs in MNCS beside the existing fix/health/report
policy modules. It is currently implemented in Rust.

## Minimal reproducer

```rust
// Desired natural expression (does not exist):
let class = policy.remediate_classify(applicability, stop, severity, budget)?;
```

## Current behavior

Rust computes the class in `src/remediate.rs` (`escalation_for_blocked`,
`escalation_for_stop`, round/budget accounting in `cmd_remediate`) while
the substantive repair, convergence, verification, and exit policies it
orchestrates already execute as MNCS (`doctor.fix`, `doctor.verify`,
`doctor.health`, `doctor.report` entrypoints, proven by
`tests/mncs_parity.rs`).

## Removal condition

Add `mncs/doctor/remediate.mncs` with a `classify` callable over the
finite input tuple, regenerate the frozen family
(`scripts/freeze-doctor-family.sh`), extend the backend corpus
(`fixtures/backend/family-corpus.json`) with classification vectors, add
generated Rust bindings plus parity tests, and delete the Rust
classification paths once parity holds. Blocked on compiler headroom for
a family freeze cycle, not on language expressiveness.

## 2026-10-01 re-evaluation (close-out run; stays open)

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

Precise removal condition now: re-pin mncs-embed to a compiler revision
whose runtime executes 0.18 cross-root imports (blocked on the active
compiler agent's finite/enum work settling — do not force), then
re-freeze with the Commons import plus thin wrappers, regenerate
bindings, extend parity tests, and delete the Rust paths.

Alternatives considered and rejected: per-file `mncs call` subprocesses
from doctor's hot path (~100ms each plus a toolchain runtime dependency
in a batch repair tool), and vendor-duplicating the law into a 0.16
doctor module (a parallel standard, forbidden by Commons ownership of
`mncs.remediation/1`).
