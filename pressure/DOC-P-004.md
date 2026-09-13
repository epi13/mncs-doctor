# DOC-P-004 — SUPERSEDED: split into scoped findings

Status: superseded (2026-09-13)

This omnibus entry grouped unrelated host-operation concerns and is
partly stale: Profile 0.16 demonstrably provides bounded file creation,
positioned writes, appends, mkdir, same-dir atomic rename, sync barriers,
single-level listing, and chunked reads (see `docs/source-profile-0.16.md`
and `spec/source-profile-registry.json` in `mncs-language`).

It is replaced by:

| ID | Capability | Status vs 0.16 |
|----|-----------|----------------|
| DOC-P-008 | recursive filesystem enumeration | absent |
| DOC-P-009 | file metadata (size/mtime/mode) | absent |
| DOC-P-010 | string-capable JSON encoding | absent (stdlib ints-only) |
| DOC-P-011 | process spawning / capture / exit status | absent |
| DOC-P-012 | symlink identity / target reads | absent beyond `other` kind |
| DOC-P-013 | temporary-file primitives | absent |
| DOC-P-014 | permission-bit access / manipulation | absent |
| DOC-P-015 | CLI argv / stdin / stdout program interface | absent |
| DOC-P-016 | TOML / project-metadata parsing | absent |
| DOC-P-019 | typed record/enum host-call ergonomics | present but rough |

Covered by 0.16 (no longer pressure): file creation, byte writes,
mkdir, same-dir atomic rename (stage-then-rename, P2-005), sync, listing,
chunked reads — all bytecode-backend-only (see backend finding in
`docs/RUST-BOUNDARY-AUDIT.md`).

Not pressure (deliberate design, reused or worked around by convention):
content hashing (stdlib `sha256.mncs` exists; byte ingress is the seam,
not the hash), sorting/bounded order statistics (stdlib `sort.mncs`;
token_set pattern adopted by `mncs/doctor/`), maps/sets (the profile
chain is linear — arithmetic suffices; no map needed), path type
(acknowledged upstream Tranche E work, not duplicated here).
