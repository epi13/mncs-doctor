# DOC-P-019 — Typed record/enum host-call ergonomics

Status: open

Category: tooling (embedding DX)

Severity: low

Frequency: per entrypoint using composite values

## Doctor workload

Calling MNCS entrypoints that take record/enum arguments from the Rust
host with canonical ABI JSON.

## Current behavior (verified 2026-09-13 by direct attempt)

Composite values cross only with exact canonical identities
(`record:mncs:0.2:record-type:<module>::<Name>::<fields…>`); anything
else fails closed as `invalid_request` with an `MNCS_VALUE_CONTRACT`
message. The failure text names the expected identity, which made the
rule discoverable — good fail-closed behavior, rough ergonomics. There
is no host-side helper to construct or pre-validate typed composite
values (no `ExecutionValue::record(module, name, fields)` constructor
resolving identities, no dry-run check).

## Workaround (adopted)

Scalar-only boundary: records/enums live inside MNCS, codes cross
(token_set pattern, stdlib precedent). Composites were proven to cross
fail-closed and then deliberately avoided at the boundary.

## Removal condition

A host-side typed-value builder (or identity-discovery query against a
compiled artifact) lands in `mncs-embed`; then re-evaluate composite
arguments for entrypoints where codes get unwieldy (e.g. transaction
op windows).

## Evidence

`tests/mncs_parity.rs::transport_mismatches_refuse_fail_closed`
(pins the fail-closed behavior); bridge-probe transcript showing the
expected-identity message guiding correction.
