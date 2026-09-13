#!/usr/bin/env bash
set -euo pipefail

# Run the checked-in pure Doctor corpora through every current value backend.
# Set MNCS_BIN when the language CLI is not on PATH.
MNCS_BIN="${MNCS_BIN:-mncs}"
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  echo "usage: MNCS_BIN=/path/to/mncs $0"
  exit 0
fi

tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/mncs-doctor-backend-matrix.XXXXXX")"
trap 'rm -rf "$tmp_root"' EXIT

backends=(
  mncs-research-bytecode
  mncs-portable-wasm-mvp
  mncs-c11
  mncs-llvm-ir
  mncs-cranelift
)

for source in "$repo_root"/mncs/doctor/*.mncs; do
  module="$(basename "$source" .mncs)"
  corpus="$repo_root/fixtures/backend/${module}-corpus.json"
  test -f "$corpus"
  for backend in "${backends[@]}"; do
    output_dir="$tmp_root/${module}/${backend}"
    mkdir -p "$output_dir"
    "$MNCS_BIN" experiment run "$source" \
      --backend "$backend" \
      --corpus "$corpus" \
      --output-dir "$output_dir" >/dev/null
    jq -e 'all(.cases[]; .status == "returned" and .expectation_met == true)' \
      "$output_dir/result.json" >/dev/null
    status="$(jq -r '.status' "$output_dir/result.json")"
    printf '%-12s %-26s %s\n' "$module" "$backend" "$status"
  done
done
