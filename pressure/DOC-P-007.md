# DOC-P-007 — No shared profile-registry data artifact

Status: workaround

Category: tooling

Severity: low

Frequency: rare (changes only when a profile ships)

## Doctor workload

`src/version.rs` mirrors the 0.1–0.16 + reserved-1.0 registry (statuses,
current marker) so planning never hardcodes "current" anywhere else.

## Current behavior

Hand-mirrored table with `registry_has_single_current` test pinning the
single-current invariant. Drift risk is low but nonzero.

## Desired behavior

Upstream ships the registry as data (JSON/TOML in `mncs-language` or via
a CLI verb) that Doctor loads at build or startup.

## Likely ownership

language (publish); Doctor (consume).
