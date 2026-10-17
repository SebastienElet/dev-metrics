#!/usr/bin/env bash
set -euo pipefail

expected_directory=$1
actual_directory=$2
release_state=$3
shopt -s dotglob nullglob
test -d "$expected_directory"
test -d "$actual_directory"
expected_files=("$expected_directory"/*)

if [[ "$release_state" != false && "$release_state" != true ]]; then
  printf 'Unknown release state: %s\n' "$release_state" >&2
  exit 1
fi
if (( ${#expected_files[@]} == 0 )); then
  printf 'No expected release assets in %s\n' "$expected_directory" >&2
  exit 1
fi

for actual_file in "$actual_directory"/*; do
  expected_file="$expected_directory/${actual_file##*/}"
  test -f "$actual_file"
  test -f "$expected_file"
  cmp "$expected_file" "$actual_file"
done

for expected_file in "${expected_files[@]}"; do
  test -f "$expected_file"
  if [[ ! -e "$actual_directory/${expected_file##*/}" ]]; then
    if [[ "$release_state" == false ]]; then
      printf 'Missing published release asset: %s\n' "${expected_file##*/}" >&2
      exit 1
    fi
    printf '%s\n' "$expected_file"
  fi
done
