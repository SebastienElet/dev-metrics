#!/usr/bin/env bash
set -euo pipefail

expected_directory=$1
actual_directory=$2
release_state=$3
shopt -s dotglob nullglob
expected_files=("$expected_directory"/*)
actual_files=("$actual_directory"/*)

if [[ "$release_state" != false ]]; then
  printf 'Existing release is not confirmed published\n' >&2
  exit 1
fi
if (( ${#expected_files[@]} == 0 )); then
  printf 'No expected release assets in %s\n' "$expected_directory" >&2
  exit 1
fi
if (( ${#expected_files[@]} != ${#actual_files[@]} )); then
  printf 'Release asset count differs: %s expected, %s actual\n' "${#expected_files[@]}" "${#actual_files[@]}" >&2
  exit 1
fi

for expected_file in "${expected_files[@]}"; do
  actual_file="$actual_directory/${expected_file##*/}"
  test -f "$expected_file"
  test -f "$actual_file"
  cmp "$expected_file" "$actual_file"
done
