# DOC-P-019 — Typed record/enum host-call ergonomics

Status: workaround

Category: tooling (embedding DX)

Severity: low

Frequency: per entrypoint using composite values

## Doctor workload

Calling MNCS entrypoints that take record/enum arguments from the Rust
host with canonical ABI JSON.

## Current behavior (verified 2026-09-12 by direct attempt)

Composite values cross with exact canonical identities
(`record:mncs:0.2:record-type:<module>::<Name>::<fields…>`); anything
else fails closed as `invalid_request` with an `MNCS_VALUE_CONTRACT`
message. The failure text names the expected identity, which made the
rule discoverable — good fail-closed behavior, rough ergonomics. A direct
compiler-produced typed-record artifact also round-tripped through the
embed boundary, proving the representation is usable when the caller has
the canonical schema. There is still no host-side helper to construct or
pre-validate typed composite values (no
`ExecutionValue::record(module, name, fields)` constructor resolving
identities, no dry-run check).

## Workaround (adopted)

Scalar-only boundary for current Doctor calls: records/enums live inside
MNCS, codes cross (token_set pattern, stdlib precedent). Composite input
and output transport is covered by a fail-closed parity probe and is
available for a future schema where scalar codes become less readable.

## Removal condition

A host-side typed-value builder (or identity-discovery query against a
compiled artifact) lands in `mncs-embed`; then re-evaluate composite
arguments for entrypoints where codes get unwieldy (e.g. transaction
op windows).

## Evidence

`tests/mncs_parity.rs::transport_mismatches_refuse_fail_closed` and the
typed-record round-trip test pin both the refusal and successful canonical
transport; `src/mncs_runtime.rs` deliberately uses scalar contracts today.
