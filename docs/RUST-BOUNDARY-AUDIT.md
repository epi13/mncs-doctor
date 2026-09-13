# Rust boundary audit

Question answered per subsystem: **why is this Rust not MNCS?**

Classes: **A** convertible now (moved, parity-tested) · **B** genuine
MNCS gap (scoped pressure, narrow shim) · **C** source feature exists,
execution/toolchain incomplete (pressure at compiler/runtime/backend) ·
**D** intentionally host-side (narrow, reasoned) · **E** temporary
bootstrap with a concrete next conversion target.

Bridge (proven 2026-09-13): `mncs-embed` (pinned rev, dev-dependency)
compiles `mncs/doctor/*.mncs` in-process (`from_source`,
`mncs-research-bytecode`) and calls scalar/array entrypoints via
`call_json`. Records/enums live inside MNCS; codes cross the boundary
(token_set pattern, stdlib precedent). Fail-closed transport is pinned:
length/signedness mismatches refuse as `invalid_request`
(`MNCS_VALUE_CONTRACT`), never silently (see `tests/mncs_parity.rs`).

## Module verdicts

### `version.rs` — A (policy) + D (data)
`compare`/`classify`/`plan_span` live in `mncs/doctor/version.mncs`
with 1:1 parity (`parity_version_*`). Registry *data* (which profiles
exist, which is current) stays host-loaded by design: it is upstream
knowledge (DOC-P-007), and MNCS takes `cur_major/cur_minor` as arguments
so no current-version assumption is baked in. Rust retained as the
shipped runtime path (E-toward-D, see below).

### `health.rs` — A (policy)
`combine`/`overall`/`check_status`/`rank` live in
`mncs/doctor/health.mncs` with parity (`parity_health_*`, full 4×4
combine matrix, 8 aggregation shapes incl. empty). Finding *collection*
(byte scanning) stays host-side with the scanner (see diagnostics).

### `migration.rs` — A (planning) + D (knowledge, execution)
`span_verdict`/`span_edges`/`enumerate`/`scan_kinds`/`plan_verdict` live
in `mncs/doctor/migration.mncs` with parity against `plan()` structure
(`parity_migration_*`). Transition *knowledge* stays host-loaded data
(DOC-P-003, correctly upstream-owned). Edge *application* (byte rewrites)
stays in the transaction executor (D, mechanism).

### `edits.rs` — A (planning) + D (application)
`pair_conflict`/`shape`/`conflicts_with_kept` live in
`mncs/doctor/edits.mncs` with parity against `EditSet::validate` over 7
overlap shapes incl. same-point inserts (`parity_edit_pair_conflict`).
Fingerprinting (sha256 over bytes) stays a host primitive: no natural
MNCS spelling at the boundary (stdlib `sha256.mncs` is pure-MNCS but
byte-string ingress is the gap). Span application stays host-side (D).

### `fix.rs` — A (policy) + E/D (execution)
`eligible`/`merge_verdict`/`stop_rule`/`seen_before` live in
`mncs/doctor/fix.mncs` with parity over the full eligibility matrix and
stopping-rule precedences (`parity_fix_*`). Provider execution (text
scanning) and loop driving stay host-side: providers need unbounded text
(E, next target after a text-ingress probe); the loop needs session
reuse across rounds (E, batch API exists — `mncs_session_call_batch`).

### `diagnostics.rs` — D (model) + E (scanner)
The `Diagnostic` envelope is the shared-contract proposal (D: it *is*
the boundary). The scanner stays Rust (E): full-file multi-pattern
hygiene scanning needs unbounded text ingress; MNCS byte views are
≤64B chunks and file ingress is chunked host reads. Next target: probe
chunk-fed scanning (u64-code windows, token_set pattern) to decide
B-vs-E with evidence instead of assertion.

### `report.rs` — A (policy) + D (rendering)
`exit_for` lives in `mncs/doctor/report.mncs` with parity over 8
command outcomes **and** cross-checked against the Rust `exit_for`
(`parity_report_exit_for`). Human rendering + JSON encoding stay
host-side (D): byte-string shaping has no compact MNCS spelling and
string-capable JSON encoding is absent (DOC-P-010).

### `verify.rs` — A (policy) + D/B (mechanism)
`delta_ok`/`compose` live in `mncs/doctor/verify.mncs` with parity incl.
a real `verify_after` cross-check (`parity_verify_compose`).
Re-diagnosis execution stays host-side (D); process spawning stays host
side (B: no spawn primitive — DOC-P-011).

### `transaction.rs` — E (policy) + C/D/B (mechanism)
Policy (ordering, staleness refusal, outcome classification) is
 MNCS-expressible in principle but **not yet attempted** (E, next target:
plan/rollback-plan entrypoints over op-code windows). Mechanism splits:
- Covered by 0.16 on the bytecode backend: create, positioned write,
  append, mkdir, same-dir atomic rename (stage-then-rename, P2-005),
  sync barrier, listing, chunked reads (C: works, but compiled backends
  refuse effects — backend finding below).
- Absent: permission-bit access, symlink-identity/target reads,
  temp-file primitives, file metadata (size/mtime), recursive
  enumeration (B: DOC-P-012…016).
No mutation moves to MNCS until the safety contract (stale-base,
symlink refusal, rollback, permission preservation) is reproducible
there. Fail closed.

### `discovery.rs` — E (policy) + B (mechanism)
Policy (exclusion decisions, extension classification over u64-code
windows, deterministic ordering) is MNCS-expressible by the same
token_set pattern (E, concrete approach documented, not yet built).
Mechanism (directory enumeration beyond one level, metadata, symlink
inspection) is B (DOC-P-012…014). `fs_list` covers one level with kinds
(file/dir/other); multi-level navigation handles are not exposed.

### `toolchain.rs` — D
Host-tool probing (PATH search, subprocess, `--help` sniffing) is
inherently host-side. The study-JSON mapping is already the narrowest
possible semantic bridge (tolerant parse, DOC201 fallback, Manual
default). No conversion target.

### `main.rs` — D (launcher) + E (orchestration)
A thin launcher (argv/env acquisition, capability setup, exit status) is
permanently D. The current handler bodies still hold orchestration that
belongs in MNCS-driven loops (planning rounds, convergence driving).
Next target: drive one command's loop from MNCS via the batch call API
once `mncs-embed` is promoted from dev- to main dependency.

## Runtime-integration unblockers (why Rust still ships the runtime)

1. Promote `mncs-embed` dev- → main dependency (measured cost: ~40 s
   first build at the pinned rev, cached after; git-rev pinning policy
   needed to match ecosystem vendoring norms).
2. Session lifecycle design (compile once per command vs per process;
   artifact digest pinning in reports for audit).
3. One command loop driven end-to-end as proof (candidate: `doctor`
   aggregation, read-only, no mutation risk).
Until then, parity tests make the MNCS core a proven drop-in and the
Rust a reference oracle — duplication with a removal condition, not
without a reason.

## Backend finding

All MNCS core modules execute on `mncs-research-bytecode` (pinned rev).
Compiled backends (C11/WASM/LLVM/Cranelift) refuse *effects*; our
modules are pure, but pure-policy portability is **untested, not
claimed**: there is no source-level multi-backend value-I/O harness
(`conformance` is predicate-based). DOC-P-017 records this (tooling,
low). No 0.16 effect primitive is used by the core, so the C-class
backend gap does not touch current execution.

## Metrics at audit time

- Rust: 5,863 LOC baseline (12 modules), retained as shipped runtime.
- MNCS implementation: 7 modules (~380 LOC, `mncs/doctor/`).
- Parity: 13 differential tests, all green; 1 semantic divergence found
  and resolved in MNCS (empty-fold identity now matches the reference).
- Transport: scalars + fixed sequences proven; records require exact
  canonical identities (fail-closed otherwise — pinned as behavior).
