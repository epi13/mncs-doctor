# DOC-P-005 — Header/profile inference is not reusable

Status: workaround

Category: tooling

Severity: medium

Frequency: occasional

## Doctor workload

`scan_header` (first non-blank/non-comment `mncs X.Y;`) mirrors upstream
`infer_source_profile`. Any divergence silently misclassifies versions and
poisons migration plans.

## Minimal reproducer

Leading-comment headers (`/* c */ mncs 0.10; …`) already disagree between
upstream layers (see `CP-0006`); Doctor's mirror adds a third
implementation of the same rule.

## Current behavior

Doctor reimplements the documented rule (~40 lines) with a regression test
pinning comment/whitespace tolerance. Divergence risk is acknowledged in
`src/diagnostics.rs` module docs.

## Desired behavior

Expose the inference as a tiny reusable unit (function over source text →
header facts) that Doctor, stage-0 tooling, and the service can share, so
`CP-0006`-class disagreements become impossible by construction.

## Likely ownership

language (extract + publish); tooling adopts.
