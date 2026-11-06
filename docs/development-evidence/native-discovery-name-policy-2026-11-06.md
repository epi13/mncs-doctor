# Native discovery name policy — 2026-11-06

## Responsibility moved

Production Doctor discovery now sends raw bounded filename and directory-name
views through the generated binding. `doctor.discovery.v1` classifies excluded
directory spellings, exact manifest names, `.mncs.json` suffixes, and `.mncs`
extensions. The implementation reuses `mncs.std.text_scan.v1` for exact byte
equality, suffix matching, and ASCII case-folded extension comparison.

The Rust production adapter no longer maps names and extensions to nominal
`DirectoryName`, `FileName`, or `FileExtension` values. It projects at most 64
leading bytes and the last 64 bytes of each name, retaining the full leading
length so an overlong exact name cannot match accidentally. The extension is
passed in its original spelling; only the native policy decides whether an
ASCII case variant is an MNCS extension.

Filesystem enumeration, path and metadata acquisition, symlink identity,
source-byte reads, root `.mncs` cache exclusion, and user-configured exclusion
list membership remain host responsibilities. The compatibility `discover()`
path keeps the independent Rust classification and traversal rules as an
oracle; production `main.rs` continues to call `discover_with_mncs_policy`.

## Verification

- `scripts/freeze-doctor-family.sh` passed. It compiled the complete imported
  family, regenerated the typed bindings, executed every checked-in family
  corpus case with its expectation met, and compiled/executed the source SHA
  kernel corpus.
- `cargo fmt --all -- --check` passed.
- `cargo test --test mncs_parity parity_discovery_fact_classification -- --exact`
  passed, covering all 17 excluded directory spellings, directory control
  cases, overlong directory and file-name prefixes, manifest precedence, long
  `.mncs.json` suffixes, uppercase `.MNCS`, and overlong extensions.
- `cargo test --test doctor production_discovery_classifies_raw_names_with_native_policy -- --exact`
  passed. It compared the native and independent Rust inventory/topology over
  the same staged filesystem, then exercised the real CLI with an excluded
  `target/` directory and an uppercase `.MNCS` source.
- `cargo test --test doctor production_discovery_uses_native_symlink_path_policy -- --exact`
  passed, confirming the migrated file classifier composes with the existing
  native symlink decision path.
- The complete Rust test suite was not run in this slice.

Each embedded policy call stays under Doctor's existing 32,768-step budget.
Directory, exact-manifest, suffix, and extension patterns share four fixed
Stdlib matcher widths, avoiding one generic instance for every spelling.
The generated family backend is 56,601,831 bytes, up 4,726,735 bytes (9.1%)
from the `main` baseline; the shared scanner was already present in the family.

## Artifact identity

- Family backend SHA-256: `107e33b673d72abc82bb06e6dabb01cf8f686ce165cf0edb07940f051f11d74b`
- Source SHA-256 backend: `733d5cd21758b9fc29d9e619e26d3eb1eb63d78efdabc0074148907eb7d6d455`
- Doctor embedded runtime revision remains `99077372dcec92e072b3cc515bb274336cd58cd2`.
- Family artifact generation used the available MNCS CLI built from
  `mncs-language` revision `f8212ed8b7478196b5293a84152154be5e0bea3f`; the
  resulting artifact was loaded and exercised by the Doctor test runtime.
