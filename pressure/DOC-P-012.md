# DOC-P-012 — No symlink identity / target reads

Status: open

Category: language (stdlib/runtime surface)

Severity: high (safety-relevant)

Frequency: common (discovery walk + transaction write guard)

## Doctor workload

Discovery skips symlinked directories by default (cycle guard) and the
transaction layer refuses symlink targets at validate *and* write time
(TOCTOU guard). Both need to distinguish symlinks and inspect them.

## Current behavior (verified 2026-09-12)

`fs_entry_kind_at` reports symlinks only as `other` (kind 2); no
readlink/target primitive exists, and 0.16 hardening treats symlinks as
untrusted (`other` entries never touched; O_NOFOLLOW tranche). There is
no granted way to observe *which* links exist as links.

## Workaround

Symlink policy stays Rust. No mutation path moves to MNCS until link
observation exists — this is a hard safety gate, not polish.

## Removal condition

Granted symlink-identity observation under `fs_root`; then reproduce
the symlink refusal tests (`preserves_permissions_and_refuses_symlinks`)
against an MNCS-driven transaction plan.

## Evidence

`docs/fs-resource-effects.md` symlink posture; transaction TOCTOU guard
in `src/transaction.rs` with passing tests.
