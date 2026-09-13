# DOC-P-014 — No permission-bit access

Status: open

Category: language (stdlib/runtime surface)

Severity: medium

Frequency: per mutated file (mode preservation)

## Doctor workload

The transaction executor records Unix mode bits and restores them on
the replacement file (and on rollback), so repair never loosens or
tightens file permissions as a side effect.

## Current behavior (verified 2026-09-12)

No mode/ownership primitive exists; 0.16 non-goals explicitly exclude a
quota/ownership model beyond host policy.

## Workaround

Mode preservation stays Rust (`file_mode`, pre-rename chmod, rollback
chmod), pinned by `preserves_permissions_and_refuses_symlinks`.

## Removal condition

Granted mode observation + set (or creation that inherits source mode
by default); then reproduce the preservation test under MNCS-driven
mutation.
