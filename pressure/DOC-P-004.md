# DOC-P-004 — MNCS cannot express Doctor's host operations

Status: workaround

Category: language

Severity: medium

Frequency: pervasive

## Doctor workload

Repository repair needs: recursive directory traversal, path manipulation,
file metadata/permissions, atomic write (temp + rename), symlink policy,
process spawning with captured output, exit codes, JSON output.

## Minimal reproducer

None of the following has a natural MNCS spelling in profiles 0.1–0.16:
walk a directory tree, read permission bits, rename a file atomically,
spawn `cargo test` and capture its status. (`docs/fs-resource-effects.md`
in `mncs-language` confirms filesystem effects are research, not product.)

## Current behavior

Doctor is implemented in Rust (the ecosystem's tooling language alongside
MNCS itself), with MNCS present as the fixture/test corpus
(`fixtures/**/*.mncs`, migration corpus) rather than the implementation
language. The boundary is narrow and documented: every language-knowledge
question (parsing, profiles, transitions) is delegated or recorded as
pressure instead of reimplemented.

## Desired behavior

No short-term demand: Doctor should stay a host-side orchestrator even as
MNCS grows effects. Long-term, any MNCS effects story should cover
least-privilege filesystem reads and process spawning so future Doctor
*policy* (not mechanism) can be expressed in MNCS.

## Likely ownership

language (effects research).

## Impact

- Correctness/safety: none — Rust is the right host today.
- MNCS-first posture: partially deferred; recorded here instead of faked
  with token MNCS files.
