# Production MNCS campaign — 2026-09

This report records the continuation campaign that moved `mncs-doctor` from
MNCS parity/reference execution to live production policy execution. The
campaign used the existing architecture and retained Rust only for host
mechanisms, upstream data not yet published, and independent reference
oracles.

## Starting baseline

| Item | Snapshot |
|---|---|
| Doctor commit | `8968e695be488c518a6e57595b80768a2e57e26f` |
| `mncs-language` main inspected at start | `85051d2a44db77133e831c7cca30eba67123a60b` |
| `mncs-language` main rechecked at finish | `e6d9238c93e356dc1a3e0a6a1aaf542ce8b0a5a` (badge-only refresh) |
| executable language implementation pinned | `a0255f8405484481b203117a659b0c1ca0fa7a5e` |
| active language branch observed | `30054cfd0b469bb326c21df3cd24e756741d7a9b` (same tree as final main; no edits made) |
| relevant unmerged language work observed | typed transport `095d87c` |
| `mncs-compiler` main | `4947dd3a0a2afcb2de27e6a2806990e44e14c041` |
| `mncs-language-service` main | `06a62ef70dfd8a147bdab22a817e6f0cb2f71d8d` |
| Forge inspected | `c045b34` (modernization branch; main `10d8ce7`) |
| Ravel inspected | `89f04bb` |
| Rust LOC / modules | 5,869 / 13 |
| MNCS Doctor LOC / modules | 414 / 7 |
| production commands using MNCS | 0 |
| parity tests | 14 |
| tests | 88 (56 unit + 18 command integration + 14 parity) |
| pressure records | 19: 12 open, 6 workaround, 1 superseded |
| E-class boundaries | 6 subsystem labels |
| B-class boundaries | 3 subsystem labels |
| C-class boundaries | 1 subsystem label |
| D-class boundaries | 10 subsystem labels |

The class counts overlap: one subsystem can be both B and C, or D and E.
They count the boundary audit labels, not lines of code.

LOC for the MNCS module metric counts `mncs/doctor/*.mncs` consistently with
the starting baseline. The 59-line `doctor_family.mncs` freeze root is
reported separately; inclusive MNCS source LOC after the campaign is 651.

The concurrent language checkout was refreshed again at campaign finish. Its
main advanced to `e6d9238`, but that final commit only refreshed the language
badge; the executable compiler/stdlib tree remained the already-tested
`a0255f8` implementation. Doctor therefore keeps the embed/artifact pin at
`a0255f8`, records both facts here, and did not claim the badge commit as a
runtime capability. The active witness branch still contained useful
typed-transport work, but it was not assumed to be shipped and Doctor did not
edit or merge it.

## Production MNCS runtime

`src/mncs_runtime.rs` introduces the single production boundary:
`DoctorMncsRuntime`.

- `mncs-embed` is now a normal dependency, pinned to the final rechecked
  language-main revision.
- The runtime embeds the ten policy sources for provenance and opens the
  checked-in 5,011,518-byte `mncs/doctor/family.backend.json`, generated from
  `mncs/doctor_family.mncs` with import resolution. One
  `mncs-research-bytecode` session serves every imported Doctor module.
- A process-wide `OnceLock` verifies/opens the family once. Calls reuse the
  retained session; production never compiles MNCS source at command startup.
- Every call requires the expected single return value and exact `i64`,
  `u64`, boolean, or fixed-sequence shape. Unknown status, unsupported calls,
  bad identity, wrong signedness/length, or unexpected return values are
  `RuntimeError`s.
- Initialization and call failures are fail-closed tool failures. The CLI
  returns exit 4 and does not silently fall back to the Rust oracle.
- Provenance contains engine (`mncs`), source profile (`0.16`), backend,
  language revision, every module's source SHA-256, artifact identity and
  artifact digest, plus the entrypoints actually executed for the report.

The imported-family freeze path is reproducible through
`scripts/freeze-doctor-family.sh` and `fixtures/backend/family-corpus.json`.
Its all-ten wrapper corpus returned all expected values on current main.
Runtime admission checks the raw artifact
SHA-256, then `Artifact::from_json` checks canonical artifact identity and
the runtime checks the expected backend.

Cold-start timing in the campaign environment was approximately 4.4 s per
separate `doctor --json` process, with approximately 40 MB peak RSS. Calls
within a process reused the family session; the filesystem probe recorded
`reused_session=true`. The lack of a safe Rust batch-call API means scanner
windows and multi-entrypoint commands still incur individual JSON calls
(DOC-P-020).

## Commands converted

### `doctor`

Host Rust acquires the workspace root, directory entries, metadata, bytes,
and optional language CLI results. MNCS performs directory inclusion and file
classification, the 64-byte BOM/newline scanner, version classification,
health status/aggregation, and report exit policy. Rust still constructs
human-readable finding records, reads TOML/JSON manifests, probes tools, and
renders terminal/JSON output.

`tests/doctor.rs::healthy_repo_reports_two_files` asserts policy provenance,
ten loaded modules, and live discovery/scanner/version/health/report
entrypoints. This is a production-path assertion, not a side-channel parity
test.

### `fix`

Host Rust providers still inspect text and construct candidate edits. MNCS
performs edit conflict policy, fix eligibility/merge decisions, re-diagnoses
through the chunk-fed scanner/version policy, convergence stop and
repeated-provider policy, transaction target policy, verification
composition, health aggregation, and report exit policy. Rust retains byte
fingerprinting, span replacement, provider text logic, temporary-file writes,
atomic rename, rollback, and permission preservation.

`tests/fix.rs::dry_run_plan_equals_applied_result` proves the applied command
uses live discovery, scanner, version, fix, edits, transaction, verification,
health, and report entrypoints while preserving dry-run equivalence and
convergence.

### `migrate`

Rust supplies the transition registry and applies the language-owned data
and transforms. MNCS performs discovery/scanner/version policy and validates
the structural migration-plan verdict from the host-supplied transition
kind window. It also performs verification, health, and report policy on the
apply path. Unknown real edges remain refused; synthetic fixture edges still
prove the engine without pretending to be historical language knowledge.

`tests/migrate.rs::apply_known_registry_migration` asserts live migration,
transaction, verification, health, and report entrypoints.

### `verify`

Rust acquires source bytes and optionally spawns the external verification
command. MNCS performs scanner/version policy, verification delta/composition,
and report exit policy. Rust owns process capture, diagnostic envelopes, and
rendering.

`tests/doctor.rs::verify_command_uses_mncs_verification_and_report_policy`
asserts live verify/report entrypoints and a passing production result.

## Rust reduction and retained responsibilities

The total Rust line count grew because a real runtime boundary, host-fact
seams, and testable fail-closed adapters are implementation work. The live
policy responsibility nevertheless moved out of the command path. Rust
reference implementations were retained only where their independent
differential value exceeds their maintenance cost.

| Module | LOC before → after | Removed from live path | Retained reason / next target |
|---|---:|---|---|
| `diagnostics.rs` | 623 → 648 | version decision and BOM/newline fact computation | UTF-8 handling, delimiter scan, diagnostic construction, optional CLI adapter; DOC-P-001/005/006 |
| `discovery.rs` | 676 → 815 | exclusion, directory verdict, file-class policy | recursive `read_dir`, metadata, symlink facts, bytes, deterministic walk; DOC-P-008/009/012/014 |
| `edits.rs` | 244 → 296 | pairwise conflict decision | bounds, SHA-256 stale-base gate, UTF-8 replacement; DOC-P-019 |
| `fix.rs` | 823 → 1,113 | live edit policy and convergence decisions | provider text scans/construction, host application, independent oracle; DOC-P-001/002/020 |
| `health.rs` | 464 → 464 | live status/overall verdict | seven check fact collectors, manifest parsing, independent oracle; DOC-P-010/016 |
| `lib.rs` | 32 → 33 | none | public module boundary |
| `main.rs` | 705 → 930 | command policy decisions | argv parsing, effect orchestration, external commands, rendering, OS exit; DOC-P-015 |
| `migration.rs` | 613 → 613 | structural path verdict | transition data, path/transform mechanism, provenance records; DOC-P-003 |
| `mncs_runtime.rs` | 0 → 1,025 | new trusted policy boundary | sessions, ABI checks, artifacts, provenance, fail-closed errors; DOC-P-018/020 |
| `report.rs` | 306 → 311 | exit-code policy | human output and string/JSON serialization; DOC-P-010/015 |
| `toolchain.rs` | 411 → 411 | none | PATH lookup, process probing, source-study adapter; DOC-P-006/011 |
| `transaction.rs` | 552 → 580 | target classification/refusal policy | native file writes, temp siblings, rename, sync, rollback, modes; DOC-P-008/009/012/013/014 |
| `verify.rs` | 203 → 216 | delta/composition verdict | external process execution, diagnostic counting, envelope; DOC-P-001/011 |
| `version.rs` | 217 → 217 | live profile classification | registry data and host version parsing; DOC-P-005/007 |

The 7,672-line total is therefore not evidence that the conversion failed;
it records the cost of making the policy boundary explicit. A future pass
should remove or shrink the Rust oracle code after the upstream diagnostic
and fix contracts make its testing value lower.

## MNCS expansion

The seven existing modules were retained and exercised through production.
They were expanded with `health.check_status_with_skip` and real command
callers. Three new modules were added:

- `transaction.mncs`: target presence, identical target, stale-base,
  symlink, and regular-file verdicts.
- `discovery.mncs`: directory traversal verdicts and manifest/source/ignore
  classification from compact host facts.
- `scanner.mncs`: stateful 64-byte byte ingress with BOM progress, LF/CRLF/
  bare-CR counts, and pending-CR carry state.

The live runtime now calls policy from all four commands. Rust supplies host
facts and performs mechanisms; it no longer decides these policy kernels in
the production path.

## Pressure findings

### Changed or newly created records

| ID | Owning layer / severity | Reproducer and impact | Workaround / removal condition |
|---|---|---|---|
| DOC-P-004 | superseded | The old omnibus waiver was already split into capability-specific records. | No action; use DOC-P-008…016 and DOC-P-019. |
| DOC-P-017 | tooling / low — fixed-upstream | `mncs experiment run` now accepts source + corpus + backend. Ten modules × five backends returned every expected value. | Keep production research-bytecode until DOC-P-021; record remains closed by the upstream value runner. |
| DOC-P-018 | tooling / low — fixed-upstream | `mncs compile` with `MNCS_LIBRARY_PATH` resolved all ten imported Doctor modules into the checked-in 5,011,518-byte `backend.json`; `experiment execute` returned all ten wrapper values. | Regenerate only through the pinned freeze script; keep raw digest, canonical identity, backend, and language-revision checks. |
| DOC-P-019 | tooling / low — workaround | Wrong canonical record identities refuse as `invalid_request`; compiler-produced typed record values also round-trip successfully. | Use scalar codes at current boundary; add a typed-value builder or identity query in `mncs-embed`. |
| DOC-P-020 | embedding API / medium — open | `Session` exposes safe `call`/`call_json`, while batch calls exist only in the unsafe C ABI; safe `TaskScope::run_batch` reopens a session per scope. Separate family-artifact processes measured 4.4 s/40 MB; chunked scanner calls remain one JSON call per window. | Open one frozen family per process and retain its session. Remove when a safe typed Rust batch/session API exists. |
| DOC-P-021 | compiler/backend/tooling / medium — open | All 50 matrix cases matched, but six modules reported `UNKNOWN` with retained unresolved obligations. | Pin research bytecode. Remove when obligations are discharged or evidence status is explicitly separated and fail-closed. |

No pressure was created merely by speculation. P-020 and P-021 came from
actual API inspection/timing and actual backend result files.

## Pressure reconciliation

| Status | Records |
|---|---|
| `workaround` | DOC-P-001, P-002, P-003, P-005, P-006, P-007, P-019 |
| `superseded` | DOC-P-004 |
| `fixed-upstream` | DOC-P-017, DOC-P-018 |
| `open` | DOC-P-008, P-009, P-010, P-011, P-012, P-013, P-014, P-015, P-016, P-020, P-021 |

The language main branch advanced during the campaign with stdlib/compiler
modernization. Fresh experiments were repeated at its final `a0255f8`
revision; P-017 and P-018 remain closed because the executable value runner
and imported-family freeze path are present on current main, not because
Doctor assumed an unmerged feature. The active language branch's
typed-transport commits were inspected but not duplicated or merged into
Doctor.

The language service currently provides structured source diagnostics,
related locations, a missing-import `MNE131` quickfix, and identity-bound
`FileEdit` results for rename/actions. It does not publish the shared
diagnostic/fix contract Doctor needs. Doctor therefore retains DOC-P-001 and
DOC-P-002 and does not reimplement semantic fixes.

The language history and documentation contain transition/state-machine
examples, but no evidence-backed historical source migration table for
0.8→0.16. All fifteen real adjacent Doctor registry edges remain unknown;
synthetic 9.x fixtures are explicitly marked fixture provenance. DOC-P-003
remains honest and actionable rather than inventing transformations.

## Profile 0.16 filesystem excursion

`tests/mncs_filesystem.rs` creates a unique scratch root and uses the actual
embedded research-bytecode session with an explicit `Grant::fs_root`:

1. create `candidate.tmp` with `0123`;
2. create `staging/`;
3. append `4567`;
4. positioned-write `X` at offset 1;
5. sync the entry;
6. same-directory rename to `candidate.mncs`;
7. list the root and read back two bounded windows.

The result was byte exact: `0X23` + `4567` and on-disk bytes
`0X234567`. The listing count was 2, the effect was recorded as `fs_write`
with `op:fs_create_file` provenance, and repeated calls reported
`reused_session=true`. The same create call without a grant returned
`unsupported` and did not create a file.

This proves real create, positioned write, append, mkdir, rename, sync,
single-level listing, and chunked reads. It does not provide recursive
listing handles, stat/mode/mtime, symlink identity/readlink, temporary-name
management, process execution, TOML, or CLI streams. It also does not prove
cross-process TOCTOU exclusion; Doctor's native transaction retains the
second symlink/fingerprint checks, temp sibling, rollback, and permission
preservation. Compiled backends were not used for effects; the pure policy
matrix is separate.

## Text ingress / scanner excursion

`doctor.scanner.v1` was fed arbitrary source bytes in 64-byte windows. Its
six-word state carries BOM prefix progress/found, LF count, CRLF count,
bare-CR count, and a pending CR across calls. The parity test deliberately
splits the UTF-8 BOM and newline pairs across chunk boundaries. The live
runtime uses the same feed/finish sequence for every discovered source.

This was enough for bounded BOM/newline hygiene. It did not make full
diagnostic scanning natural: delimiter/comment/string state and detailed
diagnostic construction still need richer state and source-text semantics;
the safe Rust session has no batch call, and byte values remain fixed-width
views. The result reduced the former scanner E boundary but exposed
DOC-P-020 instead of being treated as a blanket reason to keep all scanning
in Rust.

## Backend matrix

The checked-in corpora under `fixtures/backend/` were run with the upstream
source-level value runner. `scripts/run-backend-matrix.sh` reproduces the
50-case run. Every case returned the expected value on every backend; four
modules reported compiler `PASS`, six reported compiler `UNKNOWN` due to
retained obligations.

| Module | research bytecode | portable WASM | C11 | LLVM | Cranelift |
|---|---|---|---|---|---|
| discovery | PASS | PASS | PASS | PASS | PASS |
| edits | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN |
| fix | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN |
| health | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN |
| migration | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN |
| report | PASS | PASS | PASS | PASS | PASS |
| scanner | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN |
| transaction | PASS | PASS | PASS | PASS | PASS |
| verify | PASS | PASS | PASS | PASS | PASS |
| version | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN |

The six `UNKNOWN` results are compiler-evidence/tooling findings, not value
divergences or backend refusals. They are DOC-P-021. `conformance` was also
checked but is predicate-oriented and found no predicates in these modules;
it is not a substitute for the value corpus.

## Embedding and value transport findings

- `Artifact::from_source` still refuses imported source by design.
- The CLI freeze path resolves imports with `MNCS_LIBRARY_PATH`, emits a
  content-addressed backend artifact, and can execute it without recompiling;
  Doctor ships the all-ten-module result at
  `mncs/doctor/family.backend.json`.
- A stdlib bundle was generated and verified as a content-addressed 58-module
  source closure; the imported-family probe proved Doctor-family resolution
  separately.
- Scalar code transport is compact and strict. Wrong record identity or
  signedness/length returns `invalid_request`; tampered artifact identity
  returns `invalid_identity`.
- A compiler-produced typed record was successfully sent through a frozen
  artifact and echoed back. The representation is therefore possible, but
  manual identity reconstruction remains poor enough to retain DOC-P-019.
- Session reuse is real within a process. Safe Rust batch reuse is not yet
  available (DOC-P-020).

## Safety

The MNCS policy replacement did not weaken repository mutation invariants:

- dry-run and apply use the same MNCS-validated edit plan and host
  fingerprinted application;
- stale bases refuse before any write;
- symlink targets are refused at validation and write time;
- temporary sibling + atomic rename remains host-side;
- Unix permission bits are preserved;
- partial writes roll back completed files;
- identical targets are skipped;
- unknown migration edges and review/manual fixes remain gated;
- fixpoint, iteration budget, and oscillation policy are MNCS-backed and
  fail closed on runtime/transport errors;
- no-grant filesystem effects return unsupported and leave the scratch root
  unchanged.

## Tests and validation

`cargo test --all-targets --quiet` passed with 98 tests:

| Suite | Count |
|---|---:|
| Rust unit/runtime tests | 59 |
| command integration: doctor | 8 |
| command integration: fix | 6 |
| command integration: migrate | 5 |
| filesystem capability excursion | 1 |
| MNCS parity/transport tests | 19 |
| total Cargo tests | 98 |

Additional executed evidence:

- 10 Doctor modules passed `source-study` with no error diagnostics.
- 50 backend matrix cases returned expected values.
- imported-family freeze probe returned all three expected values.
- typed transport mismatch, invalid identity, and no-grant effect probes
  failed closed.
- production `doctor`, `fix`, `migrate`, and `verify` reports recorded live
  MNCS entrypoints and provenance.

## Metrics

| Metric | Before | After |
|---|---:|---:|
| Rust LOC | 5,869 | 7,672 |
| Rust modules | 13 | 14 |
| MNCS Doctor LOC | 414 | 592 |
| MNCS Doctor modules | 7 | 10 |
| production commands using MNCS | 0 | 4 |
| parity tests | 14 | 19 |
| total Cargo tests | 88 | 98 |
| E boundaries | 6 | 1 |
| B boundaries | 3 | 3 |
| C boundaries | 1 | 1 |
| D boundaries | 10 | 10 |
| total pressure records | 19 | 21 |
| open pressure records | 12 | 11 |
| fixed-upstream records | 0 | 2 |

The after MNCS module count/LOC excludes the 59-line family freeze root;
including that root, the after source total is 651 LOC.

The raw Rust LOC increase is the campaign's principal debt. It is mostly the
runtime boundary and explicit adapters/oracles, not a second production
policy path. The next campaign should reduce oracle code after shared
language/service contracts and frozen artifact packaging are available.

## Remaining Rust

Every substantial retained responsibility has an intentional rationale:

- `main.rs`: argv, process exit, stdout/stderr, host-effect sequencing, and
  rendering are program-entry mechanisms (DOC-P-015); policy calls are now
  delegated. A future program-entry effect could reduce the launcher.
- `discovery.rs`: recursive directory enumeration, metadata, symlink facts,
  and file bytes are host acquisition because DOC-P-008/009/012/014 remain
  open. Inclusion/classification policy is already MNCS.
- `diagnostics.rs`: UTF-8 decoding, detailed spans, delimiter/comment/string
  scanning, and semantic CLI adapter are host/tooling seams pending
  DOC-P-001/005/006. BOM/newline/version policy is MNCS.
- `health.rs`: check-fact collection includes TOML/JSON, toolchain, and
  manifest access; DOC-P-010/016 block a natural MNCS report/check payload.
  Status policy is MNCS.
- `migration.rs`: registry knowledge and byte transforms are data/mechanism
  owned by the language/tooling boundary; DOC-P-003 blocks guessing real
  historical rules. Path verdict policy is MNCS.
- `edits.rs`: SHA-256 ingress, byte-offset/UTF-8 replacement, and host text
  materialization are mechanisms; overlap policy is MNCS.
- `fix.rs`: built-in providers need source text and currently encode only
  safe hygiene; semantic providers must come from the language/service
  contract (DOC-P-001/002). Eligibility and convergence policy are MNCS.
- `transaction.rs`: native temp files, rename, sync, rollback, permission
  restoration, and TOCTOU checks are safety mechanisms covered by
  DOC-P-008/009/012/013/014. Target classification is MNCS.
- `verify.rs`: process spawning/captured output and diagnostic counting are
  host mechanisms (DOC-P-001/011); composition is MNCS.
- `report.rs`: strings, paths, terminal formatting, and JSON serialization
  remain host-side (DOC-P-010/015); exit policy is MNCS.
- `toolchain.rs`: PATH and subprocess probes are host operations (DOC-P-011);
  its source-study mapping remains the narrowest available adapter.
- `version.rs`: the profile registry is upstream data consumed by the host
  (DOC-P-007); comparison/classification policy is MNCS.
- `mncs_runtime.rs`: safe embedding, artifact/session lifecycle, ABI checks,
  provenance, and fail-closed error conversion are the trusted host bridge.
  Artifact regeneration is now scripted; DOC-P-020 is the next embedding
  reduction.

## Recommended next campaign

1. Automate artifact regeneration in release/CI builds: verify the import
   closure, compiler revision, source fingerprint, raw digest, canonical
   identity, and backend before accepting a changed family artifact.
2. Add a safe typed Rust session batch API upstream, then benchmark chunked
   scanner and multi-entrypoint command calls with it.
3. Resolve compiler obligation status (DOC-P-021) before promoting any
   compiled backend to production.
4. Establish the shared language-service/Doctor diagnostic and machine-fix
   contract (DOC-P-001/002), then remove duplicate hygiene/oracle paths that
   no longer provide independent test value.
5. Probe recursive listing handles, metadata, symlink identity, temp files,
   permissions, process effects, and string-capable report transport in
   focused upstream campaigns, keeping transaction mutation host-side until
   safety evidence is equivalent.
6. Publish evidence-backed 0.8→0.16 transition data before enabling real
   migration application; preserve unknown-edge refusal until then.
