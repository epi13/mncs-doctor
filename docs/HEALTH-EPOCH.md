# Health epochs

A health epoch (`mncs.doctor.health-epoch/1`, stored at
`.mncs/doctor/health-epoch.json`) is a validated outcome plus
fingerprints of every input that produced it. It answers one question
cheaply — "is the last validated world still the current world?" — so a
no-change `doctor` or `remediate` re-emits the proven verdict in
milliseconds instead of re-running the ~8s full path (policy-runtime
startup, discovery, per-file diagnosis, toolchain subprocesses,
knowledge probes).

The file is ignored derived state (`.mncs/doctor/` is excluded from
discovery topology). Deleting or corrupting it only costs one full pass.

## Cost model

Measured on a 2-file workspace (debug build):

| path | wall | subprocesses | policy calls |
|------|------|--------------|--------------|
| full `doctor` | ~8s (~6s runtime startup, ~0.7s toolchain probes) | ~5 | hundreds |
| epoch hit | ~10ms | 0 | 0 |

The hit path performs plain host I/O only: directory listings, file
stats, small-file hashes, and file-only knowledge probes. It never
starts the policy runtime and never spawns a subprocess (proven by
`entrypoints: []` on every hit report).

## What is fingerprinted

Every input the full path reads, in validation order (cheap checks
first; the first mismatch is the invalidation reason):

| input | how it is validated |
|-------|---------------------|
| schema, binary/report/remediation schema versions | exact match (upgrades invalidate) |
| canonical workspace root | exact match |
| discovery options identity | exact match |
| static policy identity | recomputed from embedded policy sources, no runtime |
| epoch age | ≤ 3600s; future timestamps invalidate |
| visited directory listings | re-listed and re-fingerprinted per directory |
| per-source stat metadata | size/mtime/ctime/device/inode per source |
| manifest bytes | SHA-256 per manifest (health reads manifest content) |
| `.mncs/project.json` bytes | absent/unreadable/SHA-256 |
| toolchain presence | env overrides + `PATH` + resolved paths re-resolved, binaries re-statted (no exec) |
| language/architecture knowledge | probes re-run (file I/O only) and compared exactly |
| language migration manifest | discovered path + byte digest (selects fix providers) |
| epoch digest | recomputed over inputs + state; tampering invalidates |

Identical listings plus unchanged policy/options identities imply an
identical projection (sources, manifests, skipped sets, extension
counts), so validation needs no policy callbacks: entries can only
appear inside visited directories, and skip decisions are pure
functions of unchanged listings, options, and policy.

## Soundness rules

- **Any doubt runs the full path.** Unreadable inputs, clock failure,
  schema mismatch, or an unknown reason all invalidate.
- **The digest binds inputs to state.** Hand-editing the recorded
  verdict breaks the digest and invalidates.
- **Subprocess-derived facts are validated by stat identity, not
  re-execution.** Tool `--version` strings and debugger capability
  documents are reused as last-validated facts while the binaries are
  provably untouched (ctime proves no write; inode proves no replace).
  The 1h TTL additionally bounds their staleness.
- **The frozen policy artifact contributes its pinned digest, not
  re-hashed bytes.** Runtime startup already proves bytes match the pin
  on every full path; re-hashing 8MB per hit would cost ~1s in debug
  builds for no new information. Post-build corruption therefore serves
  still-true cached verdicts until the TTL expires, then fails closed
  on the next full path.
- **Recording never fails a command.** A run that cannot record (clock
  or disk failure) still reports its validated outcome plus a note.

## What epochs never cover

| boundary | rule |
|----------|------|
| `--changed-path` | scoped state is not full state; scoped runs neither reuse nor clobber the epoch |
| `--with-language-backend` | per-file subprocess diagnosis is not fingerprinted; backend runs never reuse or record |
| `--verify-cmd` (reuse) | external results are unvalidated inputs; verify runs are always full passes (recording still applies: the proof is external-free) |
| dry-run evidence with verify | dry runs record only without `--verify-cmd` (their evidence would otherwise carry external results) |
| pending repairs | only fixpoint-proven states serve remediation reuse; a run that would repair or continue repairing always runs full |
| mode crossing | dry epochs serve dry runs only; mutating runs need non-dry fixpoint epochs |

## Command matrix

| command | reuses | records |
|---------|--------|---------|
| `doctor` | diagnostic half | diagnostic half always; remediation half when the Safe fixpoint proof holds |
| `fix` (apply and dry-run) | — (always runs; mutations need the full path) | post-state (apply) or current state (dry-run), both halves when proven |
| `remediate` | fixpoint epochs (mode-matched) | post-state, both halves when proven; dry hypotheticals for dry runs |
| `migrate`, `verify` | — | — (registry/external-command inputs are out of scope; see below) |

`fix` and `remediate` record the diagnostic half too, so the
confirming `doctor` after a repair is a hit, not another full scan.

## Re-emit semantics

- Human output is byte-stable across the fast path: the recorded
  acquisition metrics are kept, so `doctor`/`remediate` print the same
  text on a hit. Only the machine surfaces differ (`epoch_reused`,
  `epoch_digest`, the `epoch reused: …` note).
- `policy.entrypoints` is cleared on re-emit: the validating engine
  and artifacts still describe the decisions, but no entrypoint ran in
  the current process.
- Exit codes are the validated exits (a full pass over unchanged
  inputs would compute the same values deterministically).
- A `remediate` hit rewrites the evidence artifact for the current run
  (resolved fresh: explicit path, session artifact dir, or
  `.mncs/doctor/`), with empty repairs/reconciliations/diffs and the
  validated post-state diagnostics, convergence, escalations, and
  verification.

## Invalidation reasons

Stable `reason` strings (first mismatch wins, deterministic order):
`no epoch`, `epoch-unparsable`, `epoch-schema`, `doctor-version`,
`report-schema`, `root`, `options`, `policy`, `backend`,
`epoch-clock`, `expired`, `dir:<relative>`, `source:<relative>`,
`manifest:<relative>`, `project-manifest`, `toolchain`, `knowledge`,
`migrations`, `digest`.

## Why validation is host-side, not MNCS

Epoch validation exists to *avoid* starting the policy runtime, so it
cannot be policy. It is also overwhelmingly filesystem I/O (listing,
stat, small reads), which the host owns per DOC-P-008/009. The pure
compare could move into MNCS only at the cost of the startup it saves;
that trade is documented here so future work does not "fix" it.

## Extension path (deliberately out of scope)

- `migrate` planning reads an optional registry file and per-file
  migration state; fingerprinting the registry is straightforward but
  unproven against real transition rules (DOC-P-003), so migration
  neither records nor reuses yet.
- `verify` judges the tree against external commands; without them it
  is subsumed by `doctor`, with them it is ineligible by the verify
  rule above.
- A `fix` fast path (skip the planning pass when the epoch proves no
  Safe fix applies) is sound in principle but unimplemented: `fix`
  output includes per-file plans that the epoch does not store.
