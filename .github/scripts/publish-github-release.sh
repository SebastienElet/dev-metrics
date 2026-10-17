#!/usr/bin/env bash
set -euo pipefail

dist_directory=$1
release_tag=$2
repository=$3
asset_check="$(dirname "$0")/check-release-assets.sh"

if gh release view "$release_tag" --repo "$repository" >/dev/null; then
  mkdir existing
  asset_count=$(gh release view "$release_tag" --repo "$repository" --json assets --jq '.assets | length')
  case "$asset_count" in ''|*[!0-9]*) exit 1;; esac
  if (( asset_count > 0 )); then
    gh release download "$release_tag" --repo "$repository" --dir existing
  fi
  release_state=$(gh release view "$release_tag" --repo "$repository" --json isDraft --jq .isDraft)
  bash "$asset_check" "$dist_directory" existing "$release_state" > missing-assets.txt
  if [[ "$release_state" == true ]]; then
    while IFS= read -r missing_asset; do
      gh release upload "$release_tag" "$missing_asset" --repo "$repository"
    done < missing-assets.txt
    mkdir complete
    gh release download "$release_tag" --repo "$repository" --dir complete
    bash "$asset_check" "$dist_directory" complete false
    gh release edit "$release_tag" --repo "$repository" --draft=false
  fi
else
  gh release create "$release_tag" "$dist_directory"/* --repo "$repository" --verify-tag --title "$release_tag" --generate-notes
fi
