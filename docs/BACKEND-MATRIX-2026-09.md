# Doctor pure-policy backend matrix

Campaign date: 2026-09-12. Language revision: `a0255f8405484481b203117a659b0c1ca0fa7a5e`.

The upstream value runner was used directly on each Doctor source and its
checked-in corpus:

```sh
mncs experiment run mncs/doctor/version.mncs \
  --backend mncs-research-bytecode \
  --corpus fixtures/backend/version-corpus.json \
  --output-dir /tmp/doctor-backend-version
```

The same command was repeated for every source/corpus pair and each backend:

```text
mncs-research-bytecode
mncs-portable-wasm-mvp
mncs-c11
mncs-llvm-ir
mncs-cranelift
```

All 10 × 5 runs returned the expected value for every corpus case. No
backend refused these pure modules. The compiler evidence status was
`PASS` for four modules and `UNKNOWN` for six; `UNKNOWN` means the value was
returned but the compilation retained unresolved obligations, not that the
value mismatched.

| Module | bytecode | WASM | C11 | LLVM | Cranelift | value result |
|---|---|---|---|---|---|---|
| `discovery` | PASS | PASS | PASS | PASS | PASS | all expected |
| `edits` | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | all expected |
| `fix` | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | all expected |
| `health` | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | all expected |
| `migration` | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | all expected |
| `report` | PASS | PASS | PASS | PASS | PASS | all expected |
| `scanner` | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | all expected |
| `transaction` | PASS | PASS | PASS | PASS | PASS | all expected |
| `verify` | PASS | PASS | PASS | PASS | PASS | all expected |
| `version` | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | UNKNOWN | all expected |

`conformance` was also checked, but it is not the right harness for this
workload: it discovers source predicates, while Doctor's modules expose
value-returning entrypoints. The value runner is therefore the authoritative
reproducer for DOC-P-017; the unresolved compiler status is DOC-P-021.

Production remains pinned to `mncs-research-bytecode` until the unresolved
obligation result has a stable, fail-closed interpretation. The matrix is
pure policy evidence only; it does not exercise Profile 0.16 filesystem
effects, which remain bytecode-backend-specific.
