# DOC-P-010 — No string-capable JSON encoding

Status: open (narrow host seam, by design for now)

Category: stdlib

Severity: medium

Frequency: pervasive (every machine-readable report)

## Doctor workload

`report.rs` emits versioned JSON reports with messages, paths,
explanations, and provenance strings.

## Current behavior (verified 2026-09-13)

Stdlib JSON (`json.mncs`, `json_emit.mncs`) handles ints-only canonical
forms; strings are byte views without a joining/escaping/encoding
surface sufficient for report construction. The report *policy*
(exit codes) moved to `mncs/doctor/report.mncs` with parity; encoding
stays host-side (`serde_json`).

## Workaround

Split already implemented: MNCS decides, host encodes.

## Removal condition

String-capable JSON emission in stdlib (or a granted encode host-call);
then move report construction down with golden-output parity tests.

## Evidence

`tests/mncs_parity.rs::parity_report_exit_for` (policy parity green);
`docs/RUST-BOUNDARY-AUDIT.md` (report verdict).
