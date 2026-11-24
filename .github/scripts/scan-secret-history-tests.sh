#!/usr/bin/env bash
set -euo pipefail

scanner=${1:?usage: scan-secret-history-tests.sh gitleaks-binary}
script_directory=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/cma-secret-history.XXXXXXXX")
cleanup() {
  case "$fixture_root" in
    */cma-secret-history.*) rm -rf -- "$fixture_root" ;;
    *) exit 1 ;;
  esac
}
trap cleanup EXIT

git init --quiet --initial-branch=accepted "$fixture_root/repository"
cd "$fixture_root/repository"
git config user.name 'History scanner fixture'
git config user.email 'scanner@example.invalid'
git -c commit.gpgsign=false commit --quiet --allow-empty -m 'Clean candidate'

expect_scan() {
  local expected=$1 actual=0
  bash "$script_directory/scan-secret-history.sh" "$scanner" . \
    >"$fixture_root/scan.log" 2>&1 || actual=$?
  if [[ "$actual" -ne "$expected" ]]; then
    printf 'history scan returned %s; expected %s\n' "$actual" "$expected" >&2
    cat "$fixture_root/scan.log" >&2
    exit 1
  fi
}

expect_scan 0
git switch --quiet --create unmerged
# Synthetic and generated inside the disposable fixture, never a credential.
# A token-shaped value matches a dedicated rule on every run. A random value
# under the generic rule is occasionally filtered, which made this self-test
# fail about once in 500 runs.
token=$(openssl rand -base64 64 | LC_ALL=C tr -dc 'A-Za-z0-9')
printf 'token = "ghp_%s"\n' "${token:0:36}" >fixture.txt
git add fixture.txt
git -c commit.gpgsign=false commit --quiet -m 'Synthetic finding on another branch'
expect_scan 1

git switch --quiet accepted
expect_scan 0
git switch --quiet unmerged
git rm --quiet fixture.txt
git -c commit.gpgsign=false commit --quiet -m 'Remove fixture from the current tree'
expect_scan 1

git switch --quiet accepted
git -c commit.gpgsign=false merge --quiet --no-ff unmerged -m 'Merge history with removed finding'
expect_scan 1
printf 'Secret history regressions passed: clean, unmerged, current, removed and merged ancestry.\n'
