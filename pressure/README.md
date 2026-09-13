# Pressure records

Doctor is a language-pressure source: implementing repository repair against
the current MNCS toolchain exposed genuine capability gaps. Each finding
follows the ecosystem convention (`CP-XXXX` in `mncs-compiler/pressure`,
`LS-P-XXX` in `mncs-language-service/pressure`):

- identifier, status, severity, category, frequency
- the real Doctor workload that exposed it
- minimal reproducer
- current behavior and workaround (with why the workaround is insufficient)
- desired behavior and likely ownership
- impact and evidence

Status vocabulary: `open` (confirmed, no workaround), `workaround` (Doctor
ships a stopgap), `fixed-upstream` (resolved by a language/toolchain change;
entry retained as archive), `rejected-as-pressure` (not a language concern).

## Index

| ID | Title | Severity | Status |
|----|-------|----------|--------|
| [DOC-P-001](DOC-P-001.md) | No stable programmatic diagnostics API | high | workaround |
| [DOC-P-002](DOC-P-002.md) | No shared structured fix/edit contract | high | workaround |
| [DOC-P-003](DOC-P-003.md) | No published version-transition rules | medium | workaround |
| [DOC-P-004](DOC-P-004.md) | MNCS cannot express Doctor's host operations | medium | workaround |
| [DOC-P-005](DOC-P-005.md) | Header/profile inference is not reusable | medium | workaround |
| [DOC-P-006](DOC-P-006.md) | CLI diagnostic output has no machine contract | low | open |
| [DOC-P-007](DOC-P-007.md) | No shared profile-registry data artifact | low | workaround |

Upstream echoes (not duplicated here, referenced): `CP-0006`
(stage-0 envelope inference vs leading comments),
`LS-P-003` (leaf provenance preservation), `LS-P-005`
(symbolic edits must stay behind a boundary — Doctor's `EditSet`
fingerprint gate follows the same principle).
