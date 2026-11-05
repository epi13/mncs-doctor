# DOC-P-005 — Header/profile inference is not reusable

Status: workaround

Category: tooling

Severity: medium

Frequency: occasional

## Doctor workload

`scan_header` (first non-blank/non-comment `mncs X.Y;`) and upstream
`infer_source_profile` concern related source facts. Doctor now runs its
documented shallow header and module-presence scan in MNCS, but the scanner is
still Doctor-owned and does not call or share Language's inference. Any
behavioral divergence can silently misclassify versions and poison migration
plans.

## Minimal reproducer

Leading-comment headers (`/* c */ mncs 0.10; …`) already disagree between
upstream layers (see `CP-0006`). Doctor's scanner is now independently
implemented in MNCS, so it no longer adds a third host-language copy, but it
does not eliminate the ownership and parity risk across tools.

## Current behavior

Doctor's `doctor.scanner.v1` implementation recognizes its documented rule;
an independent host reference remains in tests for differential checks.
Language's Rust `infer_source_profile` remains separate, so cross-tool
divergence risk remains.

## Desired behavior

Expose the inference as a tiny reusable unit (function over source text →
header facts) that Doctor, stage-0 tooling, and the service can share, so
`CP-0006`-class disagreements become impossible by construction.

## Likely ownership

language (extract + publish); tooling adopts.
