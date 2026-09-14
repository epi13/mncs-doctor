# Selective Doctor surfaces

Doctor separates discovery from the amount of source work requested. Discovery
builds the authoritative inventory once; `--changed-path` then selects a
validated subset for diagnostics, safe fixes, migration planning/application,
and verification reports.

```text
repository inventory -> explicit changed paths -> bounded Doctor operation
                                      \-> scope note + selected/available counts
```

Examples:

```sh
mncs-doctor doctor --root . --changed-path src/module.mncs --json
mncs-doctor fix --root . --changed-path src/module.mncs --dry-run --json
mncs-doctor migrate --root . --to latest --plan \
  --changed-path src/module.mncs --json
```

The default is repository scope. A path must resolve inside the discovered
workspace and match an inventoried MNCS source; Doctor fails closed instead of
silently widening or narrowing an invalid request. Broad repository or family
audits remain appropriate for explicit migration campaigns, profile upgrades,
releases, and canonical merge verification.

Doctor is not the semantic impact authority. Ravel obtains the compiler's
`mncs.semantic-impact/1` projection and produces a verification plan for
mncs-test. Doctor's scope is a deterministic migration/health boundary and its
report is evidence that downstream orchestration can reference.

## Incremental inventory evidence

Doctor persists a digest-bound inventory at `.mncs/doctor/inventory.json`.
The cache binds the canonical workspace root, discovery options, static MNCS
policy provenance, inventory identity, and per-source size/mtime signatures. A
valid `--changed-path` request refreshes only the requested source entries and
reuses the remaining inventory after checking those signatures; the JSON and
human reports expose `files_rescanned`, `files_reused`, `cache_identity`, and
`invalidation_reason`. A missing/invalid cache, an unreported source mutation,
or a new source topology falls back to one full discovery with an explicit
reason. The cache never treats a stale serialized inventory as authoritative.
Repository discovery remains the default for initialization, audits, profile
upgrades, releases, and explicit canonical boundaries.
