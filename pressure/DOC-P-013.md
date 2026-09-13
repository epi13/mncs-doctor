# DOC-P-013 — No temporary-file primitives

Status: open

Category: language (stdlib/runtime surface)

Severity: medium

Frequency: per mutated file (transaction executor)

## Doctor workload

Every file write goes to a temp sibling + atomic rename so a crash
never leaves a half-written file; temp names must not collide and must
clean up on failure paths.

## Current behavior (verified 2026-09-13)

No temp-file intrinsic exists; 0.16 offers `fs_create_file` (exclusive —
refuses when present, never overwrites), which covers collision refusal
but not unique-name generation or temp-lifecycle conventions.

## Workaround

Temp+rename stays in the Rust executor.

## Removal condition

A granted temp-file convention (unique names within the grant root,
executors clean up) or an blessed exclusive-create retry pattern;
then reproduce the atomicity tests in MNCS-driven execution.

## Evidence

`src/transaction.rs` (`sibling_tmp`, rename, no-temp-survival test).
