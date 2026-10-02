# DOC-P-023 — Health policy window fixed at 8 checks; no dedicated stdlib check id

Status: open

Category: tooling

Severity: medium

Frequency: once per new check (structural ceiling)

## Doctor workload

Stage F adds standard-library composition diagnostics (missing/invalid/
stale pin, incompatible profiles, unresolved project imports). These
belong to a dedicated `stdlib-health` check id so reports, exit policy,
and automation can address stdlib composition directly.

## Minimal reproducer

Register a ninth check in `health::run_all_checks` and run
`mncs-doctor doctor --json`:

```text
mncs-doctor: error: MNCS health policy failed (fail-closed):
mncs_value_contract: health policy accepts at most 8 checks, got 9
```

## Current behavior

The MNCS policy window is `[Status; 8]` (`mncs/doctor/health.mncs`
`OverallInput`), enforced by host value-contract guards
(`mncs_runtime.rs`: health guard, packed resize, report guard,
statuses resize) and pinned by `empty_workspace_passes_with_toolchain_note`
(`results.len() == 8`). The policy ships as a sha-pinned compiled
artifact (`mncs/doctor/family.backend.json`), so widening the window
requires recompiling policy with the pinned toolchain and rotating the
artifact identity — a trust-root change, not a casual edit.

## Workaround in place

Stdlib composition findings ride `toolchain-health` (see
`health::stdlib_composition_findings`), and the structured
`toolchain.stdlib` status carries module/profile/identity facts.
No diagnostic is lost; only the dedicated check id is missing.

## What unblocks

Widen the policy window (8 → 9+) in `health.mncs`, recompile
`family.backend.json` with the pinned compiler revision, rotate
`FAMILY_ARTIFACT_SHA256`, widen the host guards, and promote the
stdlib findings to a `stdlib-health` check id.
