# Ambient remediation

`mncs-doctor remediate` is the machine-native repair entrypoint. Where
`fix` is an operator tool (`doctor` → read diagnosis → run `fix` →
inspect result), `remediate` absorbs that loop: detect, classify, repair,
validate, record, and stay quiet.

```text
$ mncs-doctor remediate --target .
remediation: repaired=2 reconciled=1 degraded=0 blockers=0
remaining: none
evidence: .mncs/doctor/remediation-evidence.json
```

## Command

```text
mncs-doctor remediate [--target <dir>] [--dry-run] [--budget <n>]
                      [--evidence-path <file>] [--verify-cmd "<cmd>"]
                      [--changed-path <file> ...] [--json] [--quiet]
```

`--root` stays accepted as the historical alias for `--target`.

## Remediation classes

| Class | Meaning | Examples |
|---|---|---|
| `safe_automatic` | Deterministic problem and repair, validated after mutation. Applied without involving the caller. | Trailing whitespace, final newline, CRLF→LF, BOM removal, mechanically-safe canonical module identities |
| `bounded_reconciliation` | The invariant is known but recovery takes work. Bounded, idempotent, validated; failure escalates. | Stale/corrupt inventory cache regenerated; further repair rounds exposed by earlier repairs (max 3 rounds) |
| `escalation` | Needs intent, judgment, or work outside safe repair. Carries the smallest sufficient evidence (`code:path` + one action). | Review/manual-classified fixes, parse errors, oscillation, edit conflicts, budget exhaustion, verification failure |

Ambient remediation is always Safe-only. `--proven` does not exist here:
semantically-proven repairs need explicit direction through `fix`, and
review/manual classes are never applied by `remediate`.

## Output contract

Stdout is always terse (`mncs.remediation/1`, the family contract owned
by MNCS-Commons): provider identity, scope echo, counts, repair records
with before/after fingerprints, reconciliation records, escalation ids
(capped at 64 inline with `remaining_truncated`), one evidence pointer,
budget accounting, and the verification verdict. Full detail (per-file
diagnostics, diffs, convergence traces, verification outcome, MNCS
policy provenance) goes to the evidence artifact
(`mncs.remediation-evidence/1`), never to the working context.

Evidence routing, in order:

1. explicit `--evidence-path`;
2. `$MNCS_ENV_SESSION_ARTIFACT_DIR/remediation-evidence.json` when invoked
   through `mncs-environment` (the calling session owns the trail);
3. `<root>/.mncs/doctor/remediation-evidence.json` (git-ignored,
   regenerable derived state).

`--dry-run` predicts without mutating: records carry `validated: false`
and `dry_run: true`; evidence is still written.

## Budgets and idempotence

- `--budget <n>` (default 256) caps files repaired per run. Files beyond
  the budget escalate as `budget-exhausted:<path>`; a re-run continues
  where the previous run stopped.
- At most 3 apply rounds per run. Rounds past the first are recorded as
  `converge:round-N` reconciliations.
- A second run over repaired state reports `repaired=0` and exits quietly.
  Nothing is re-applied, nothing is re-reported.

## Safety

`remediate` reuses the `fix` safety core unchanged: fingerprint validation
before and during writes, temp+rename commits, symlink refusal, conflict
refusal, rollback on partial failure, and post-mutation verification
through MNCS policy (`doctor.verify`, `doctor.health`, `doctor.report`
entrypoints). See `docs/SAFETY.md`. Exit codes follow `docs/EXIT-CODES.md`.

What `remediate` refuses:

- review/manual-classified edits (escalates with the provider id);
- files that fail to parse or decode (escalates `DOC106`/`DOC108`-class ids);
- oscillating or conflicting edit sets (escalates, never force-applies);
- anything past the file/round budgets (escalates, never silently truncates);
- reporting success when composed verification fails (escalates
  `verification-failed` as a blocker).

## Environment integration

`mncs-environment` discovers Doctor as a provider through
`.mncs/project.json` invocation descriptors:

- `mncs-doctor:repository-diagnostics` → `remediate`-free `doctor` (read-only);
- `mncs-doctor:repository-remediation` → `remediate` (write effects: the
  session must hold a claim on the target scope);
- `mncs-doctor:repository-validation` → `verify` (write effects: the
  command can execute an external verification command).

Availability is honest: the capabilities bind only when the release binary
(`target/release/mncs-doctor`) is built. When invoked through a session,
Doctor writes its evidence artifact into the session-provided artifact
directory and returns only the terse envelope on stdout.
