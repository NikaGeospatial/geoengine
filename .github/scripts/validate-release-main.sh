#!/usr/bin/env bash
set -euo pipefail

error_prefix="${1:-Release drift detected}"

git fetch --no-tags origin main
main_sha="$(git rev-parse origin/main)"
release_sha="${GITHUB_SHA}"

if [[ "${GITHUB_REF:-}" == refs/tags/* ]]; then
  git fetch --no-tags origin "${GITHUB_REF}"
  release_sha="$(git rev-parse FETCH_HEAD^{commit})"
fi

echo "origin/main=${main_sha}"
echo "release=${release_sha}"

if [[ "$main_sha" != "$release_sha" ]]; then
  echo "ERROR: ${error_prefix}. origin/main ($main_sha) does not match release commit ($release_sha)." >&2
  exit 1
fi
