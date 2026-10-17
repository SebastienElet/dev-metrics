#!/usr/bin/env bash
set -euo pipefail

expected_directory=$1
actual_directory=$2
check_mode=$3
shopt -s dotglob nullglob
test -d "$expected_directory"
test -d "$actual_directory"
expected_files=("$expected_directory"/*)

if [[ "$check_mode" != draft && "$check_mode" != published && "$check_mode" != complete ]]; then
  printf 'Unknown asset check mode: %s\n' "$check_mode" >&2
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
  if [[ "$check_mode" == complete ]]; then
    cmp "$expected_file" "$actual_file"
  fi
done

for expected_file in "${expected_files[@]}"; do
  test -f "$expected_file"
  if [[ ! -e "$actual_directory/${expected_file##*/}" ]]; then
    if [[ "$check_mode" != draft ]]; then
      printf 'Missing release asset: %s\n' "${expected_file##*/}" >&2
      exit 1
    fi
  fi
done
