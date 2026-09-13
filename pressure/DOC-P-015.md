# DOC-P-015 — No CLI argv / stdin / stdout program interface

Status: open

Category: language + tooling

Severity: medium (caps the thin-launcher goal)

Frequency: structural (one decision: can the launcher be MNCS?)

## Doctor workload

`main.rs` should ideally shrink to argv acquisition, capability setup,
core invocation, and exit-status return. That requires MNCS programs to
receive arguments and emit bytes/status.

## Current behavior (verified 2026-09-13)

Inputs cross only as `ExecutionRequest.arguments` + grants (≤64 bytes
per grant blob); there is no argv/stdin surface and no stdout/exit-code
surface — observation is `ExecutionResult` consumed by the host. A
standalone `mncs-doctor` binary whose `main` is MNCS is therefore not
expressible; the host shell is load-bearing, not incidental.

## Workaround

Thin Rust launcher (D) invoking library logic; MNCS core reached in
tests via `call_json`, at runtime not yet (see audit unblockers).

## Removal condition

A program-entry effect (granted argv ingress, byte-stream egress, exit
status) lands; then re-evaluate the launcher split with a `main.mncs`
witness.

## Evidence

`crates/mncs-embed/src/lib.rs` (`call_json`, grants ≤64B);
`docs/RUST-BOUNDARY-AUDIT.md` (main.rs verdict).
