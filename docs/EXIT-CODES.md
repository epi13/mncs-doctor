# Exit codes

| Code | Name | Meaning |
|------|------|---------|
| 0 | healthy | No actionable findings; verification (if run) passed |
| 1 | findings | Findings present: repairable, informational, or planned changes shown; nothing requires human judgment to stay safe |
| 2 | review-required | Some items are `review`/`manual` (or migration edges `unknown`): automatic repair would be unsafe |
| 3 | verification-failed | Post-mutation verification failed (new errors, non-idempotence, or a failing `--verify-cmd`) |
| 4 | tool failure | Internal error, bad arguments, blocked migration apply, transaction failure |

Notes:

- `fix` on a repo with remaining informational findings (e.g. sealed
  profiles) exits 1 after a successful apply — the mutation succeeded;
  the repo simply is not fully current. Check `verification.passed` in
  JSON for the mutation verdict.
- `migrate --plan` over unrecorded edges exits 2 (review the path, do not
  auto-apply); `migrate --apply` across them exits 4 without mutating.
- All commands print the numeric code and its meaning in JSON
  (`exit_code`, `exit_meaning`).
