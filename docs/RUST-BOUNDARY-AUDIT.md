# Rust boundary audit

Question answered per subsystem: **why is this Rust not MNCS?**

Classes: **A** convertible now (moved, parity-tested) · **B** genuine
MNCS gap (scoped pressure, narrow shim) · **C** source feature exists,
execution/toolchain incomplete (pressure at compiler/runtime/backend) ·
**D** intentionally host-side (narrow, reasoned) · **E** temporary
bootstrap with a concrete next conversion target.

Bridge (proven 2026-09-12): `mncs-embed` is a pinned normal dependency.
`DoctorMncsRuntime` opens the checked-in `mncs/doctor/family.backend.json`
(`mncs-research-bytecode`) once per process, retains one session for all twelve
imported policy modules, and uses the generated Rust binding for nominal
records and finite values. The binding submits the expected interface
identity on every call. The generated binding now also covers the
multi-argument bounded-byte and delimiter-state scanner calls, so production has no generic
`call_json` transport path. Fail-closed transport is pinned:
length/signedness mismatches refuse as `invalid_request`
(`MNCS_VALUE_CONTRACT`), never silently. Reports include source/artifact
identities and the entrypoints actually used (see `tests/mncs_parity.rs` and
`tests/doctor.rs`).

## Module verdicts

### `version.rs` — A (policy) + D (data)
`compare`/`classify`/`plan_span` live in `mncs/doctor/version.mncs`
with 1:1 parity (`parity_version_*`). Registry *data* (which profiles
exist, which is current) stays host-loaded by design: it is upstream
knowledge (DOC-P-007), and MNCS takes `cur_major/cur_minor` as arguments
so no current-version assumption is baked in. Rust retains only the registry
data and host-facing version parsing; production classification is MNCS.

### `health.rs` — D (fact collection and text; verdicts are native)
`combine`/`overall`/`check_status`/`rank` live in
`mncs/doctor/health.mncs` with parity (`parity_health_*`, full 4×4
combine matrix, 8 aggregation shapes incl. empty). Status and severity are
nominal finite values in the generated binding; host code still collects
findings and renders their text, while the scanner owns byte-level decisions
(see diagnostics). Health collection now returns facts and findings only;
`run_all_checks` applies the native `check_status_with_skip` and `overall`
kernel before any result reaches a consumer, and returns an error if that
policy cannot run. CLI, remediation, migration, and the public report exit
helper all use the same verified runtime. The duplicate host status folds and
report exit mapping have been retired. The health-check identity is also a
native finite value: Doctor no longer decides in Rust which check promotes
informational findings; `version-drift` promotion is selected inside
`doctor.health.v1`. Rust maps its report check identifier to the finite value,
collects counts, and renders finding text.

### `migration.rs` — A (planning) + D (knowledge, execution)
`span_verdict`/`span_edges`/`enumerate`/`scan_kinds`/`plan_verdict` live
in `mncs/doctor/migration.mncs` with parity against `plan()` structure
(`parity_migration_*`). Transition kind and plan verdict are nominal finite
values in the generated binding. Transition *knowledge* stays host-loaded data
(DOC-P-003, correctly upstream-owned). Edge *application* (byte rewrites)
stays in the transaction executor (D, mechanism).

### `edits.rs` — A (planning) + D (application)
`pair_conflict`/`shape`/`conflicts_with_kept` live in
`mncs/doctor/edits.mncs` with parity against `EditSet::validate` over 7
overlap shapes incl. same-point inserts (`parity_edit_pair_conflict`).
Conflict and shape results are nominal finite values in the generated binding.
Fingerprinting (sha256 over bytes) stays a host primitive: no natural
MNCS spelling at the boundary (stdlib `sha256.mncs` is pure-MNCS but
byte-string ingress is the gap). Span application stays host-side (D).

### `fix.rs` — A (policy) + E/D (execution)
`eligible`/`merge_verdict`/`stop_rule`/`seen_before` live in
`mncs/doctor/fix.mncs` with parity over the full eligibility matrix and
stopping-rule precedences (`parity_fix_*`). Applicability, merge, and seen
results are nominal finite values in the generated binding. Production planning, edit
conflict validation, and loop decisions use MNCS; providers still acquire
text and construct candidate edits in Rust. Chunk-fed BOM/newline and
delimiter scans are MNCS-backed. Rust maps the native mismatch offset to a
Unicode-aware span and constructs `DOC106` or the explicit `DOC109` capacity
finding. The missing safe Rust batch API is DOC-P-020.

### `diagnostics.rs` — D (model) + A (hygiene scanning) + E (semantic diagnostics)
The `Diagnostic` envelope is the shared-contract proposal (D: it *is*
the boundary). A stateful chunk-fed scanner crosses 64-byte views and produces
BOM/newline counts, shallow `mncs X.Y;` header facts, `module ...;` presence,
delimiter balance, and byte offsets in production. One composed native call
per 64-byte source window carries both newline and header state, so header
facts reuse the existing byte-ingress pass. Its 1,016-entry opener stack
reports capacity exhaustion as `DOC109`, never a balanced result. Rust retains
Unicode span projection, diagnostic text, and semantic language
diagnostics; independent host header and delimiter implementations are
compiled only for tests and parity checks. Header parsing remains Doctor's
hygiene contract; reusable semantic source-profile inference is still an
upstream Language pressure (DOC-P-005), while deeper language diagnostics
remain upstream-owned (DOC-P-001/020).

### `report.rs` — A (policy) + D (rendering)
`exit_for` lives in `mncs/doctor/report.mncs` with parity over 8
command outcomes **and** cross-checked against the Rust `exit_for`
(`parity_report_exit_for`). Human rendering + JSON encoding stay
host-side (D): the semantic result is a nominal `ExitDecision` in the
generated binding; byte-string shaping has no compact MNCS spelling and
string-capable JSON encoding is absent (DOC-P-010).

### `verify.rs` — A (policy) + D/B (mechanism)
`delta_ok`/`compose` live in `mncs/doctor/verify.mncs` with parity incl.
a real `verify_after` cross-check (`parity_verify_compose`). Delta and
composition results are nominal `VerificationVerdict` values in the
generated binding.
Re-diagnosis execution stays host-side (D); process spawning stays host
side (B: no spawn primitive — DOC-P-011).

### `transaction.rs` — A (policy) + C/D/B (mechanism)
Target presence, identical-target, stale-base, symlink, and regular-file
classification are decided by `doctor.transaction.v1` during every live
validation. The Rust reference remains for differential tests. Mechanism
splits:
- Covered by 0.16 on the bytecode backend: create, positioned write,
  append, mkdir, same-dir atomic rename (stage-then-rename, P2-005),
  sync barrier, listing, chunked reads (C: works, but compiled backends
  refuse effects — backend finding below).
- Absent: permission-bit access, symlink-identity/target reads,
  temp-file primitives, file metadata (size/mtime), recursive
  enumeration (B: DOC-P-012…016).
Unsafe mutation is still Rust. The policy call is fail-closed and does not
authorize a write by itself; stale-base, symlink refusal, rollback, and
permission preservation remain host invariants.

### `discovery.rs` — A (policy) + B (mechanism)
Production `doctor.discovery.v1` decides directory exclusion/traversal,
symlink path handling, and file classes from host-acquired facts. Its native
symlink policy classifies file, directory, other, and unresolved targets;
applies file/directory follow settings; and resolves directory-link depth and
cycle outcomes. The Rust walker retains target metadata and directory
identity acquisition, deterministic ordering, recursion, and file reads.
The independent Rust decisions remain available as a differential reference.
Mechanism (directory enumeration beyond one level, metadata, symlink
inspection) is B (DOC-P-008…014). `fs_list` covers one level with kinds
(file/dir/other); multi-level navigation handles are not exposed.

### `architecture_knowledge.rs` — A (validation policy) + D (fact acquisition)
`doctor.architecture.v1` owns architecture-model schema and SHA-256 identity
checks, claims-header drift, capability lookup, canonical/shadow path and
retirement-state drift, stale-path verdicts from host-observed existence, and
wire-contract membership. Text equality reuses
`mncs.std.text_scan.v1::compare` from Stdlib. Rust reads bounded JSON facts,
observes path existence, passes those facts through the generated binding, and
renders diagnostics. The native policy returns the matched capability index,
so the host does not repeat capability identity matching to explain a verdict.
Independent fixtures cover valid projections, uppercase hexadecimal
identities, malformed schemas/identities, all drift classes, missing and
unknown IDs, and fail-closed text bounds.

### `toolchain.rs` — D
Host-tool probing (PATH search, subprocess, `--help` sniffing) is
inherently host-side. The study-JSON mapping is already the narrowest
possible semantic bridge (tolerant parse, DOC201 fallback, Manual
default). No conversion target.

### `main.rs` — D (launcher/effects) + A (orchestration boundary)
The launcher acquires argv, workspace paths, filesystem bytes/metadata,
process results, renders human/JSON output, and returns the OS exit code.
All four command handlers initialize the runtime and route meaningful
discovery, version, health, edit, fix, migration, verification, transaction,
scanner, and report decisions through it. Remaining orchestration is host
control flow around those policy calls, not a hidden Rust policy fallback.

## Production-runtime result

The runtime integration is now shipped. `mncs-embed` is a normal dependency;
`OnceLock` verifies/opens the checked-in twelve-module family artifact once per
process and reuses its session. Every call requires one returned value, checks
the exact scalar/sequence shape, rejects unsupported/failing policy calls, and
records the shared artifact identity/digest. Initialization and call failures
return tool failure (exit 4); there is no silent Rust fallback.

The source-to-family freeze path was adopted: `mncs/doctor_family.mncs` imports
all twelve Doctor modules, `mncs compile` + `MNCS_LIBRARY_PATH` produces the
checked-in family artifact, and `mncs experiment execute` returns all twelve
wrapper expectations. `src/mncs_runtime.rs` also validates the raw artifact
digest before `Artifact::from_json` validates the canonical identity.

## Backend finding

All twelve MNCS core modules execute on `mncs-research-bytecode` (pinned rev).
The upstream source-level value runner executed every module on research
bytecode, portable WASM, C11, LLVM, and Cranelift; all values matched. Six
modules retain compiler `UNKNOWN` evidence because unresolved obligations
remain (DOC-P-021), so production does not silently promote compiled
backends. No 0.16 effect primitive is used by the core; the separate
filesystem probe is intentionally bytecode-only.

## Metrics at campaign close

See `docs/CAMPAIGN-2026-09-PRODUCTION.md` for the measured before/after
table. Net Rust LOC increases because the first production runtime boundary,
host-fact seams, and independent oracle wrappers were added; the live
responsibility moved is policy, not merely line count. The Rust duplicates
remain only where their independent differential value is explicit.
