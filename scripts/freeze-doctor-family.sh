#!/usr/bin/env bash
set -euo pipefail

# Regenerate the checked-in imported Doctor policy family. The compiler and
# stdlib roots are explicit so the resulting artifact is reproducible from a
# pinned language checkout rather than from ambient project state.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# A linked worktree's Git common directory identifies the family checkout;
# resolve sibling repositories from there instead of from `.worktrees/<name>`.
git_common_dir="$(git -C "$repo_root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
family_root="$repo_root"
if [[ "$git_common_dir" == */.git ]]; then
  family_root="${git_common_dir%/.git}"
fi
language_root="${MNCS_LANGUAGE_ROOT:-$family_root/../mncs-language}"
stdlib_root="${MNCS_STDLIB_ROOT:-$family_root/../mncs-stdlib}"
commons_root="${MNCS_COMMONS_ROOT:-$family_root/../MNCS-Commons}"
if [[ -n "${MNCS_BIN:-}" ]]; then
  mncs_bin="$MNCS_BIN"
elif [[ -x "$language_root/target/debug/mncs" ]]; then
  mncs_bin="$language_root/target/debug/mncs"
elif [[ -x "$language_root/target/release/mncs" ]]; then
  mncs_bin="$language_root/target/release/mncs"
else
  echo "native mncs compiler not found under $language_root/target; set MNCS_BIN" >&2
  exit 2
fi
# Stdlib is the canonical owner of the current mncs.std modules. The Language
# library mirrors several of those identities; including both roots makes the
# compiler reject an otherwise valid import as an ambiguous duplicate.
library_path="$repo_root/mncs:$stdlib_root/library:$commons_root/src/mncs_commons/mesh"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/mncs-doctor-freeze.XXXXXX")"
trap 'rm -rf -- "$tmp_dir"' EXIT

MNCS_LIBRARY_PATH="$library_path" \
  "$mncs_bin" compile "$repo_root/mncs/doctor_family.mncs" \
  --emit backend \
  --target mncs-research-bytecode \
  --output-dir "$tmp_dir" >/dev/null

MNCS_LIBRARY_PATH="$library_path" \
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
MNCS_LIBRARY_PATH="$library_path" \
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

# Keep the build-time provenance pin aligned with the artifact that just
# passed its execution corpus. Updating only after both corpus checks prevents
# a failed freeze from blessing an unverified family artifact.
python3 - "$repo_root/mncs/doctor/source-hash-inputs.json" \
  "$repo_root/mncs/doctor/family.backend.json" <<'PY'
import hashlib
import pathlib
import re
import sys

manifest = pathlib.Path(sys.argv[1])
artifact = pathlib.Path(sys.argv[2])
digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
text = manifest.read_text(encoding="utf-8")
updated, count = re.subn(
    r'("family_artifact_sha256"\s*:\s*")[0-9a-f]{64}',
    lambda match: f"{match.group(1)}{digest}",
    text,
    count=1,
)
if count != 1:
    raise SystemExit("source hash manifest has no unique family artifact pin")
manifest.write_text(updated, encoding="utf-8")
PY

sha256sum "$repo_root/mncs/doctor/source-sha256.backend.json"
sha256sum "$repo_root/mncs/doctor/family.backend.json"
