# DOC-P-011 — No process spawning / capture / exit status

Status: open

Category: language (effects)

Severity: medium

Frequency: occasional (`verify --verify-cmd`, toolchain probing)

## Doctor workload

Post-repair verification shells out to project commands (`cargo test`)
and toolchain probes execute `mncs`/`cargo`/`ravel` with captured
output and status codes.

## Current behavior (verified 2026-09-12)

The profile registry contains no spawn/process/env/exit feature;
0.16 non-goals exclude a conventional runtime model. There is no
effect declaration a process-spawning intrinsic could attach to.

## Workaround

`verify::run_external` and toolchain probing stay in the Rust substrate;
the *composition policy* (delta/idempotence/external-channel verdicts)
moved to `mncs/doctor/verify.mncs` with parity.

## Removal condition

A granted process-spawn effect (allowlist + capture + status, mirroring
the `fs_root` authority pattern) lands; until then this pressure
justifies the narrowest host seam, not a language redesign.

## Evidence

Registry grep for spawn/process/env/exit: zero hits (method in
DOC-P-004); `parity_verify_compose` pins the policy/host split.
