#!/usr/bin/env bash
set -euo pipefail

# Regenerate the checked-in imported Doctor policy family. The compiler and
# stdlib roots are explicit so the resulting artifact is reproducible from a
# pinned language checkout rather than from ambient project state.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
mncs_bin="${MNCS_BIN:-mncs}"
language_root="${MNCS_LANGUAGE_ROOT:-$repo_root/../mncs-language}"
stdlib_root="${MNCS_STDLIB_ROOT:-$repo_root/../mncs-stdlib}"
commons_root="${MNCS_COMMONS_ROOT:-$repo_root/../MNCS-Commons}"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/mncs-doctor-freeze.XXXXXX")"
trap 'rm -rf -- "$tmp_dir"' EXIT

MNCS_LIBRARY_PATH="$repo_root/mncs:$stdlib_root/library:$language_root/library:$commons_root/src/mncs_commons/mesh" \
  "$mncs_bin" compile "$repo_root/mncs/doctor_family.mncs" \
  --emit backend \
  --target mncs-research-bytecode \
  --output-dir "$tmp_dir" >/dev/null

MNCS_LIBRARY_PATH="$repo_root/mncs:$stdlib_root/library:$language_root/library:$commons_root/src/mncs_commons/mesh" \
  "$mncs_bin" abi "$repo_root/mncs/doctor_family.mncs" >"$tmp_dir/doctor.abi.json"
python3 "$language_root/scripts/generate_host_bindings.py" \
  --abi "$tmp_dir/doctor.abi.json" \
  --language rust \
  --output "$tmp_dir/doctor_version.rs"
python3 - "$tmp_dir/doctor_version.rs" "$repo_root/src/generated/doctor_version.rs" <<'PY'
import pathlib
import re
import shutil
import sys

generated = pathlib.Path(sys.argv[1])
current = pathlib.Path(sys.argv[2])
identity_pattern = re.compile(
    r'pub const INTERFACE_IDENTITY:\s*&str\s*=\s*"([0-9a-f]{64})"'
)

def interface_identity(path):
    match = identity_pattern.search(path.read_text(encoding="utf-8"))
    if match is None:
        raise SystemExit(f"generated host binding has no interface identity: {path}")
    return match.group(1)

if not current.exists() or interface_identity(generated) != interface_identity(current):
    shutil.copyfile(generated, current)
PY

cp "$tmp_dir/backend.json" "$repo_root/mncs/doctor/family.backend.json"
execute_output="$tmp_dir/execute.json"
"$mncs_bin" experiment execute "$repo_root/mncs/doctor/family.backend.json" \
  "$repo_root/fixtures/backend/family-corpus.json" >"$execute_output"
jq -e 'all(.[]; .status == "returned" and .expectation_met == true)' \
  "$execute_output" >/dev/null
sha256sum "$repo_root/mncs/doctor/family.backend.json"

# Doctor uses the canonical Stdlib stream implementation for source
# provenance. Keep its artifact separate so health-epoch hits do not need to
# open the larger policy-family runtime.
sha256_dir="$tmp_dir/source-sha256"
mkdir -p "$sha256_dir"
MNCS_LIBRARY_PATH="$repo_root/mncs:$stdlib_root/library:$language_root/library:$commons_root/src/mncs_commons/mesh" \
  "$mncs_bin" compile "$stdlib_root/library/std/sha256.mncs" \
  --emit backend \
  --target mncs-cranelift \
  --output-dir "$sha256_dir" >/dev/null
cp "$sha256_dir/backend.json" "$repo_root/mncs/doctor/source-sha256.backend.json"
sha256_execute_output="$tmp_dir/source-sha256-execute.json"
"$mncs_bin" experiment execute "$repo_root/mncs/doctor/source-sha256.backend.json" \
  "$stdlib_root/examples/execution/sha256-corpus.json" >"$sha256_execute_output"
jq -e 'all(.[]; .status == "returned" and .expectation_met == true)' \
  "$sha256_execute_output" >/dev/null
sha256sum "$repo_root/mncs/doctor/source-sha256.backend.json"
