# Compiler/VM health

`compiler-vm-health/1` diagnoses independently selected Language reference,
compiler producer and canonical VM runtime. It checks actual provider contracts,
compiler build-input freshness, exact executable bytes, standalone artifact
admission and a bounded request. A product with corrupt references/bytes or stale
source/producer input refuses. Declarations without an execution proof remain
`unknown`.

`--smoke` uses the provider-owned nested-call fixture, the selected direct
producer, a session artifact cache, a finite 100-step request and the known answer
7. Environment's normal readiness service consumes this result. `--product FILE
--request FILE` checks a caller's real frozen product instead. `--cache` is required
for standalone smoke unless Environment supplies its reserved session artifact
root; no checkout-local output or ambient compiler substitution is needed.

PASS proves this selected bounded execution and identity chain. It does not certify
all compiler features, all backends or independently attest a prebuilt executable.
CP-0024 project JIT and CP-0025 portable-WASM execution are separate optional
backend observations and are not required by the canonical VM smoke.

Doctor transports compiler/VM-owned facts; it does not lower, interpret SSA,
reconstruct compiler source maps or decide storage/placement semantics.
