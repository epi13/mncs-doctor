# Pressure records

Doctor is a language-pressure source. Each finding follows the ecosystem
convention (`CP-XXXX` in `mncs-compiler/pressure`, `LS-P-XXX` in
`mncs-language-service/pressure`): identifier, status, severity,
workload, desired expression, observed behavior, workaround and its
limits, ownership, removal condition, evidence.

Status vocabulary: `open` (confirmed) · `workaround` (stopgap shipped) ·
`superseded` (split or landed elsewhere) · `fixed-upstream` (resolved;
retained as archive) · `rejected-as-pressure` (not a language concern).

Reconciled 2026-09-13 against `mncs-language`/`mncs-compiler`/
`mncs-language-service`/`mncs-forge-mcp`/RAVEL current main, Profile
0.16 scope doc, registry, and live execution. DOC-P-004 was split (see
its file); DOC-P-006's index line was stale and is corrected below.

## Index

| ID | Title | Severity | Status |
|----|-------|----------|--------|
| [DOC-P-001](DOC-P-001.md) | No stable programmatic diagnostics API | high | workaround |
| [DOC-P-002](DOC-P-002.md) | No shared structured fix/edit contract | high | workaround |
| [DOC-P-003](DOC-P-003.md) | No published version-transition rules | medium | workaround |
| [DOC-P-004](DOC-P-004.md) | Omnibus host-operations waiver | — | superseded (split) |
| [DOC-P-005](DOC-P-005.md) | Header/profile inference is not reusable | medium | workaround |
| [DOC-P-006](DOC-P-006.md) | Study-JSON unversioned + file-local | low | workaround |
| [DOC-P-007](DOC-P-007.md) | No shared profile-registry data artifact | low | workaround |
| [DOC-P-008](DOC-P-008.md) | No recursive filesystem enumeration | high | open |
| [DOC-P-009](DOC-P-009.md) | No file metadata (size/mtime/mode) | medium | open |
| [DOC-P-010](DOC-P-010.md) | No string-capable JSON encoding | medium | open |
| [DOC-P-011](DOC-P-011.md) | No process spawning / capture / exit status | medium | open |
| [DOC-P-012](DOC-P-012.md) | No symlink identity / target reads | high | open |
| [DOC-P-013](DOC-P-013.md) | No temporary-file primitives | medium | open |
| [DOC-P-014](DOC-P-014.md) | No permission-bit access | medium | open |
| [DOC-P-015](DOC-P-015.md) | No CLI argv / stdin / stdout interface | medium | open |
| [DOC-P-016](DOC-P-016.md) | No TOML / project-metadata parsing | low | open |
| [DOC-P-017](DOC-P-017.md) | No source-level multi-backend value harness | low | open |
| [DOC-P-018](DOC-P-018.md) | `from_source` refuses `use` (no embed freeze) | low | open |
| [DOC-P-019](DOC-P-019.md) | Typed record/enum host-call ergonomics | low | open |

Covered by Profile 0.16, therefore **not** pressure (verified, not
assumed): file creation, positioned writes, appends, mkdir, same-dir
atomic rename, sync barriers, single-level listing, chunked reads —
bytecode-backend-only (see `docs/RUST-BOUNDARY-AUDIT.md`).

Deliberate design, therefore not pressure: content hashing (stdlib
`sha256.mncs`; ingress is the seam), sorting (stdlib `sort.mncs`;
token_set pattern adopted), maps (unneeded — linear profile chain),
path type (upstream Tranche E, referenced not duplicated).

Upstream echoes (referenced, not duplicated): `CP-0006`
(envelope inference), `LS-P-003` (leaf provenance), `LS-P-005`
(edits behind a boundary — `EditSet`'s fingerprint gate follows it).
