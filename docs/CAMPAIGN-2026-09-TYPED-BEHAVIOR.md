# Typed Doctor policy boundary — September 2026

This addendum records the second part of the 2026 Doctor campaign. It does
not replace `CAMPAIGN-2026-09-PRODUCTION.md`, which describes the original
runtime adoption and its measured baseline.

## Objective

Remove manually synchronized semantic integer maps from the production Rust
host layer while preserving legitimate host responsibilities: filesystem
and process access, bounded serialization, rendering, and transaction
mechanics.

## Delivered boundary

The Doctor family now exposes nominal finite values and records for health,
report, verification, edit, fix, and migration policy. The checked-in
`src/generated/doctor_version.rs` is generated from the language-owned ABI
metadata rather than handwritten transport structs. It records:

- module identity `doctor.family.v1`;
- the callable interface identity
  `1af5a86bad2a5cf60cdb6ab541f2c11454797c0ec5a47e3c695f58a92eb6e523`;
- typed-call schema revision;
- generator and binding content identities.

Every generated wrapper submits the expected interface identity, so a stale
binding fails deterministically instead of allowing a changed enum, field,
parameter, or return type to be reinterpreted.

The binding generator now also emits multi-argument callable wrappers and
bounded byte-view transport. Doctor's scanner `feed(bytes, state)` and
`finish(state)` calls therefore use generated Rust bindings; the production
runtime no longer formats a generic typed-call JSON envelope for scanner
ingress.

The host still transports natural numeric facts such as version coordinates,
scanner bytes, counts, offsets, and fixed-size windows. Discovery name and
extension facts now also cross as nominal generated finite values. It no
longer maps semantic health, exit, verification, migration, edit, fix, or
discovery decisions through manually maintained integer tables.

## Verification

The retyped family artifact was regenerated from `mncs/doctor_family.mncs`
and the generated Rust binding was regenerated from its ABI. `cargo check
--all-targets`, the library tests, the Doctor integration suites, and the
19-case MNCS parity suite pass. The parity suite also sends an integer where
the native `TransitionKind` finite value is required and verifies the typed
transport rejects it.

The production runtime and parity tests use the same generated nominal
types. Rust remains responsible for acquiring facts and applying effects;
MNCS remains the authority for the policy decision.
