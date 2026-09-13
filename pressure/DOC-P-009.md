# DOC-P-009 — No file metadata (size/mtime/mode)

Status: open

Category: language (stdlib/runtime surface)

Severity: medium

Frequency: common (inventory + transaction safety)

## Doctor workload

Inventory records byte length, permission bits, and symlink identity;
the transaction executor preserves permission bits and refuses symlink
targets. Change detection and preservation both need metadata.

## Current behavior (verified 2026-09-13)

No `stat`-family intrinsic exists in the 0.16 scope doc or the profile
registry. Length is derivable by chunked reads (wasteful but possible);
mtime/mode bits are not observable at all.

## Workaround

Metadata stays in the Rust substrate (`discovery.rs`, `transaction.rs`).

## Removal condition

Granted metadata observation (at least kind+size+mode, ideally mtime)
under the `fs_root` authority model.

## Evidence

Registry + `docs/source-profile-0.16.md` enumeration (see DOC-P-008
for method); `transaction.rs` permission-preservation tests pin the
behavior that must one day be reproducible in MNCS.
