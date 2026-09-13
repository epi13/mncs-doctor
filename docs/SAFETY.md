# Safety model

## Applicability levels

| Level | Meaning | Auto-applied? |
|-------|---------|---------------|
| `safe` | Formatting normalization, unambiguous modernization with unchanged semantics, deterministic canonicalization | yes (default) |
| `semantically_proven` | Semantic-directed transforms with established equivalence | only with `--proven` (no built-in provider claims this yet) |
| `review` | Likely migration with possible semantic implications | never automatically; `migrate` needs `--allow-review` |
| `manual` | Explainable but not safely transformable | never; explanation + suggested action only |

Default automatic repair applies `safe` only and must never silently
rewrite uncertain semantic code.

## Transaction guarantees

- **Dry-run fidelity**: `summarize_diff` is a pure function of
  (old, new) content; integration tests assert dry-run fingerprints equal
  post-apply fingerprints.
- **Pre-validation**: every overwrite target must exist, match its
  recorded SHA-256, and not be a symlink — checked for *all* files before
  *any* write.
- **Atomicity per file**: temp sibling + rename; a crash cannot leave a
  half-written file. No temp files survive success (tested).
- **Deterministic order**: ascending relative path.
- **Rollback**: in-memory originals restore completed writes on partial
  failure; the error reports completed/total/failed-at plus rollback
  status (`ok (N restored)` or `INCOMPLETE …`).
- **Preservation**: unchanged files are never touched (identical writes
  skipped); Unix permission bits preserved; newlines/encoding pass through
  except for bytes a fix explicitly produced.
- **Symlinks**: directory symlinks are not descended (default) and
  symlink targets are refused at validate *and* write time (TOCTOU guard).

## Convergence guards

- Iteration budget (16 per file), oscillation detection (a provider firing
  twice stops the loop with `oscillation`, applied once), per-fix conflict
  skipping (a conflicting fix is recorded and skipped, not fatal), and
  stale-base refusal at every application.

## What Doctor does NOT guarantee

- Semantic equivalence of repairs: built-ins are formatting-only; anything
  deeper is `review`/`manual` by construction.
- Cross-process atomicity: concurrent external writers are detected
  (fingerprint mismatch → refusal), not locked out. Re-run to converge.
- Full-repo atomicity: per-file atomicity + rollback, not a single global
  commit. Git remains the outer transaction — commit before running `fix`
  or `migrate --apply` on precious trees.
