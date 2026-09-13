#!/usr/bin/env bash
set -euo pipefail

# Regenerate the checked-in imported Doctor policy family. The compiler and
# stdlib roots are explicit so the resulting artifact is reproducible from a
# pinned language checkout rather than from ambient project state.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
mncs_bin="${MNCS_BIN:-mncs}"
language_root="${MNCS_LANGUAGE_ROOT:-$repo_root/../mncs-language}"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/mncs-doctor-freeze.XXXXXX")"
trap 'rm -rf -- "$tmp_dir"' EXIT

MNCS_LIBRARY_PATH="$repo_root/mncs:$language_root/library" \
  "$mncs_bin" compile "$repo_root/mncs/doctor_family.mncs" \
  --emit backend \
  --target mncs-research-bytecode \
  --output-dir "$tmp_dir" >/dev/null

cp "$tmp_dir/backend.json" "$repo_root/mncs/doctor/family.backend.json"
execute_output="$tmp_dir/execute.json"
"$mncs_bin" experiment execute "$repo_root/mncs/doctor/family.backend.json" \
  "$repo_root/fixtures/backend/family-corpus.json" >"$execute_output"
jq -e 'all(.[]; .status == "returned" and .expectation_met == true)' \
  "$execute_output" >/dev/null
sha256sum "$repo_root/mncs/doctor/family.backend.json"
