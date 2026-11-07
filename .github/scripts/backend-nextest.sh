#!/usr/bin/env bash
# Source this file for nextest_build, or execute it with nextest run/list args.
set -euo pipefail

nextest_repo_root=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
nextest_build_args=(--workspace --locked)
case "${CMA_NEXTEST_REUSE+x}:${CMA_NEXTEST_REUSE-}" in
  :)
    if [[ "${CI:-false}" == true ]]; then
      echo 'CI must use the extracted Backend test archive' >&2
      exit 1
    fi
    ;;
  x:1)
    nextest_metadata="$nextest_repo_root/api/target/nextest"
    # Absolute paths embedded by env! cannot be remapped at runtime. Reject
    # missing metadata and relocated checkouts before starting any fixtures.
    python3 - "$nextest_repo_root/api" "$nextest_metadata" <<'PY'
import json
from pathlib import Path
import sys

workspace, metadata = map(Path, sys.argv[1:])
with (metadata / "cargo-metadata.json").open() as source:
    cargo = json.load(source)
with (metadata / "binaries-metadata.json").open() as source:
    binaries = json.load(source)
if (cargo["workspace_root"] != str(workspace)
        or cargo["target_directory"] != str(workspace / "target")
        or binaries["rust-build-meta"]["target-directory"] != str(workspace / "target")
        or binaries["rust-build-meta"].get("build-directory") not in (None, str(workspace / "target"))):
    raise SystemExit("Backend archive must be extracted at its original checkout and target paths")
PY
    nextest_build_args=(--binaries-metadata "$nextest_metadata/binaries-metadata.json"
      --cargo-metadata "$nextest_metadata/cargo-metadata.json")
    ;;
  *) echo 'CMA_NEXTEST_REUSE must be unset (local build) or 1 (extracted archive)' >&2; exit 1 ;;
esac

nextest_build() (
  local action=${1:?expected run or list}
  shift
  local build_args=("${nextest_build_args[@]}")
  case "$action" in
    run) build_args+=(--no-tests fail) ;;
    list) ;;
    *) echo 'expected nextest run or list' >&2; exit 2 ;;
  esac
  cd "$nextest_repo_root/api"
  exec cargo nextest "$action" "${build_args[@]}" "$@"
)

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  nextest_build "$@"
fi
