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
