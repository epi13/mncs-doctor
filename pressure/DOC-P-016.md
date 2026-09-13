# DOC-P-016 — No TOML / project-metadata parsing

Status: open

Category: stdlib

Severity: low

Frequency: per workspace (`mncs-forge.toml`, manifests)

## Doctor workload

Manifest health checks (`version` field presence) and future metadata
upgrades need to read `mncs-forge.toml` and `.mncs.json` sidecars.

## Current behavior (verified 2026-09-12)

No TOML primitive exists in the registry or stdlib; manifest checks
stay host-side (`check_manifest_health`).

## Workaround

TOML stays Rust; JSON sidecars stay host-parsed (`serde_json`).

## Removal condition

Granted TOML read (or a manifest-schema host-call returning codes);
then move manifest-health policy into `mncs/doctor/` with fixture
parity.

## Evidence

`src/health.rs::check_manifest_health`; registry grep (zero toml hits).
