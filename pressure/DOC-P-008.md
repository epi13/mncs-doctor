# DOC-P-008 — No recursive filesystem enumeration

Status: open

Category: language (stdlib/runtime surface)

Severity: high (blocks MNCS-native discovery)

Frequency: pervasive (every repository walk)

## Doctor workload

`discovery.rs` walks the workspace tree: multi-level descent, exclusion
matching per directory, symlink-directory policy, deterministic ordering.

## Desired natural MNCS expression

```mncs
effect fs_list authorized_by cap;
// list roots, then descend into subdirectories of interest —
// needs per-directory listing handles across levels.
```

## Current behavior (verified 2026-09-12 against `mncs-language` main)

`fs_list_count()`, `fs_entry_name_at(i)`, `fs_entry_kind_at(i)`
(0=file, 1=dir, 2=other), `fs_generation()` cover a **single level**
(`docs/fs-resource-effects.md:37-40`). No recursive primitive exists;
the 0.16 scope doc lists recursive delete among explicit non-goals.
Multi-level navigation (open a listing for a subdirectory entry, with
containment kept) is not exposed, so a bounded worklist walker cannot be
assembled from documented parts. Attempt status: capability analysis
against the scope doc + registry (no recursion intrinsic present). The
multi-module freeze prerequisite is no longer the blocker (DOC-P-018 is
fixed-upstream); the missing listing-handle/recursive surface is the
remaining reason the walker stays host-side.

## Workaround

Discovery mechanism stays Rust (`discovery.rs`); only the *policy*
(exclusions, classification, ordering) is slated for MNCS windows.

## Why insufficient

The largest remaining Rust module cannot move until enumeration exists.

## Removal condition

A bounded multi-level enumeration primitive (or per-directory listing
handles composable under the `fs_root` grant) lands with bytecode
realization; then build the MNCS walker with differential tests.

## Evidence

- Registry: `fs_list*` only; no walk/recursion feature
  (`spec/source-profile-registry.json`).
- `docs/RUST-BOUNDARY-AUDIT.md` (discovery verdict: E-policy / B-mechanism).
