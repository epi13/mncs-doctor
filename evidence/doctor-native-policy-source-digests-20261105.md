# Doctor native policy-source identity

## Responsibility moved

Doctor previously used the Rust SHA-256 implementation to calculate source
digests for its family policy and twelve policy modules, then hash the
aggregate policy identity. The health-epoch cache calls that identity before
opening Doctor's policy runtime.

The canonical Stdlib `mncs.std.sha256.v1` implementation now computes all of
those values through its pinned Cranelift artifact during Cargo's incremental
build. `build.rs` passes source bytes in bounded 1024-byte chunks and writes
the resulting digests, module table, provenance, and policy identity into
Cargo's generated output. Doctor reads those generated constants at runtime;
it no longer opens a second MNCS runtime or runs host SHA-256 over policy
source or identity material.

`mncs/doctor/source-hash-inputs.json` is the source list and identity contract.
The build checks its module declarations against both the source files and
the `doctor.*` callable modules in the frozen Doctor family artifact. It
rejects mismatched artifact pins, backend identity, or call provenance. The
generated policy identity covers the policy profile/backend, Language
revision, frozen family artifact pin, SHA artifact pin/backend, family source
digest, and every policy-module key and digest.

Rust SHA-256 remains for the existing runtime integrity check on the frozen
Doctor policy artifact. Build-time host SHA-256 only checks the raw frozen
artifact pins. The native digest artifact is included in the test build for
an independent pin assertion; it is not embedded in the production Doctor
binary. Older serialized `PolicyProvenance` records remain readable, with
the new digest metadata defaulting to empty.

## Frozen inputs and generated identity

- Stdlib artifact file SHA-256: `733d5cd21758b9fc29d9e619e26d3eb1eb63d78efdabc0074148907eb7d6d455`
- Stdlib artifact identity: `mncs:compiler:backend-artifact:654acc7b28c2a2a7ea9ae3d748d6a10b78bd3918640580ab58f7b6f940167f8d`
- Stdlib artifact payload SHA-256: `6a48c78f32e0dde090183c6c65209d7084d0403e05c6a6a515359b746e6417c9`
- Backend: `mncs-cranelift`
- Doctor family artifact file SHA-256: `2cface9ba9ca00ff7210eb32294bd78e459c55a119728cfcbafbab0dab7d421b`
- Generated Doctor policy identity: `fa940fe5d6bc2f96560a1d1220b3d318980c50ffd18c8a172025acb01d796d27`

## Verification

Commands ran with the canonical Doctor Cargo target at
`/home/epi13/Documents/Projects/mncs-doctor/target`.

- `cargo test --lib mncs_runtime::tests -- --test-threads=1`: 8 passed.
  This includes an independent host SHA-256 oracle for all generated module
  and family digests, the aggregate identity, full runtime provenance, the
  artifact pin, and deserialization of legacy provenance.
- `cargo test --test health_epoch doctor_second_run_reuses_validated_epoch -- --exact`:
  passed. The actual CLI's second Doctor invocation reused its validated epoch.
- `cargo test --test health_epoch doctor_source_mutation_invalidates_epoch -- --exact`:
  passed. Changing a source still forces a full validation pass.
- `scripts/freeze-doctor-family.sh`: passed; regenerated and executed the
  Doctor family artifact and the Stdlib SHA-256 corpus through MNCS.
- `cargo fmt --all` and `git diff --check`: passed.

The repository-wide `cargo test` command was not run in this slice; these
focused runtime and epoch integration checks cover the changed behavior.
