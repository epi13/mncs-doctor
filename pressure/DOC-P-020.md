# DOC-P-020 — No Rust `Session` batch-call API

Status: open

Category: tooling (embedding API)

Severity: medium

Frequency: every chunked or multi-entrypoint production call

## Doctor workload

Production Doctor retains one `mncs_embed::Session` for the complete imported
policy family and must call scanner `feed` once per 64-byte window, then
`finish`. Commands also call several scalar policy entrypoints in sequence.
Session reuse avoids recompilation, but there is no safe public Rust batch
surface to amortize request validation and transport overhead.

## Reproducer

At `mncs-language` `a0255f8`, `mncs-embed::Session` exposes `call` and
`call_json`. A safe `TaskScope::run_batch` also exists, but it reopens a
session from frozen artifact bytes for each scope run; it is not a batch
method on a retained `Session` and cannot express the scanner's dependent
`feed` → next-state → `feed` chain. The same crate exports
`mncs_session_call_batch` only through the unsafe C ABI. Doctor's safe Rust
runtime cannot use that ABI without creating a second handle/lifetime/
serialization boundary.

## Observed impact

The 64-byte scanner is genuinely stateful and works, but each chunk is a
separate JSON call. A cold `mncs-doctor doctor` process verifying/opening the
5.0 MB family artifact measured approximately 4.4 seconds and 40 MB peak RSS
in the campaign environment. Subsequent calls within one process reuse the
session; separate CLI invocations necessarily pay artifact admission/opening.

## Workaround

`DoctorMncsRuntime` verifies/opens the frozen family once per process, retains
the session in a process-wide `OnceLock`, and fails closed on every
call/return contract failure. The scanner uses fixed windows and the policy
runtime records every entrypoint in report provenance.

## Removal condition

Expose a safe, typed Rust batch/session API with deterministic ordering,
per-call results, shared artifact identity, and request-level failure
classification. Re-run the scanner and command performance probes before
changing the runtime lifecycle.

## Evidence

- `src/mncs_runtime.rs` session cache and chunk-fed scanner.
- `crates/mncs-embed/src/lib.rs` `Session::call_json` versus the C-only
  `mncs_session_call_batch`.
- Campaign timing: separate family-artifact `doctor --json` processes took
  approximately 4.4 s and 40 MB peak RSS; the session-reuse filesystem test
  observed `reused_session=true`. The final `mncs-language` main commit
  `e6d9238` was badge-only; the API evidence remains from executable tree
  `a0255f8`.
