#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
readonly SCRIPT_DIR
readonly VALIDATOR="$SCRIPT_DIR/release-notes.sh"
TEST_TMP=$(mktemp -d "${TMPDIR:-/tmp}/release-notes-tests.XXXXXX")
readonly TEST_TMP
readonly STDOUT_FILE="$TEST_TMP/stdout"
readonly STDERR_FILE="$TEST_TMP/stderr"

trap 'rm -r "$TEST_TMP"' EXIT

passed=0
status=0
output=""
errors=""

build_body() {
  local before=$1
  local gap=$2
  local new_note=$3
  local fixed_note=$4
  local changed_note=$5
  local after=$6

  if [ -n "$before" ]; then
    printf '%s\n\n' "$before"
  fi

  printf '## Release Notes\n'
  if [ -n "$gap" ]; then
    printf '%s\n' "$gap"
  fi
  printf '\n### What\047s New\n- %s\n' "$new_note"
  printf '\n### What\047s Fixed\n- %s\n' "$fixed_note"
  printf '\n### What\047s Changed\n- %s\n' "$changed_note"

  if [ -n "$after" ]; then
    printf '\n## Reviewer Notes\n%s\n' "$after"
  fi
}

run_validator() {
  local body=$1

  if printf '%s' "$body" | "$VALIDATOR" >"$STDOUT_FILE" 2>"$STDERR_FILE"; then
    status=0
  else
    status=$?
  fi
  output=$(<"$STDOUT_FILE")
  errors=$(<"$STDERR_FILE")
}

fail_test() {
  local name=$1
  local detail=$2

  printf 'not ok - %s\n%s\n' "$name" "$detail" >&2
  exit 1
}

pass_test() {
  local name=$1

  passed=$((passed + 1))
  printf 'ok %d - %s\n' "$passed" "$name"
}

assert_accepts() {
  local name=$1
  local body=$2
  local expected=$3

  run_validator "$body"
  if [ "$status" -ne 0 ]; then
    fail_test "$name" "validator exited $status:"$'\n'"$errors"
  fi
  if [ "$output" != "$expected" ]; then
    fail_test "$name" "unexpected extracted notes:"$'\n'"$output"
  fi
  pass_test "$name"
}

assert_rejects_version() {
  local name=$1
  local body=$2
  local version=$3
  local additional_error=${4:-}

  run_validator "$body"
  if [ "$status" -eq 0 ]; then
    fail_test "$name" "validator accepted version $version"
  fi
  case "$errors" in
    *"remove the version number '$version'"*) ;;
    *) fail_test "$name" "missing version error for $version:"$'\n'"$errors" ;;
  esac
  if [ -n "$additional_error" ]; then
    case "$errors" in
      *"$additional_error"*) ;;
      *) fail_test "$name" "missing additional error '$additional_error':"$'\n'"$errors" ;;
    esac
  fi
  pass_test "$name"
}

assert_rejects_versions() {
  local name=$1
  local body=$2
  shift 2
  local expected_count=$#
  local actual_count
  local version

  run_validator "$body"
  if [ "$status" -eq 0 ]; then
    fail_test "$name" "validator accepted multiple versions"
  fi
  actual_count=$(printf '%s\n' "$errors" | grep -c '^::error::remove the version number ' || true)
  if [ "$actual_count" -ne "$expected_count" ]; then
    fail_test "$name" "expected $expected_count version errors, found $actual_count:"$'\n'"$errors"
  fi
  for version in "$@"; do
    case "$errors" in
      *"remove the version number '$version'"*) ;;
      *) fail_test "$name" "missing version error for $version:"$'\n'"$errors" ;;
    esac
  done
  pass_test "$name"
}

expected=$(printf "### What's New\n- %s\n\n### What's Fixed\n- %s\n\n### What's Changed\n- %s" \
  "Added deterministic release validation." \
  "Fixed release note extraction." \
  "Updated the release gate.")

review_body=$(build_body \
  "Reviewer context belongs outside the published section." "" \
  "Added deterministic release validation." \
  "Fixed release note extraction." \
  "Updated the release gate." \
  "Reviewers should inspect the workflow logs.")
assert_accepts \
  "valid reviewer prose is accepted and excluded from extracted notes" \
  "$review_body" "$expected"

number_body=$(build_body \
  "Resolves #711 on 2026-08-27 after 42 checks at 127.0.0.1 and 10.20.30.40:8080; ratio 3.14." "" \
  "Added deterministic release validation." \
  "Fixed release note extraction." \
  "Updated the release gate." "")
assert_accepts \
  "issue references, dates, ordinary numbers, and IPv4 addresses are accepted" \
  "$number_body" "$expected"

assert_rejects_version \
  "prefixed version in reviewer prose is rejected" \
  "$(build_body "Reviewer target: v1.2.3." "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "v1.2.3"

assert_rejects_version \
  "uppercase-prefixed version in reviewer prose is rejected" \
  "$(build_body "Reviewer target: V1.2.3." "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "V1.2.3"

assert_rejects_version \
  "plain version in reviewer prose is rejected" \
  "$(build_body "Reviewer target: 1.2.3." "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "1.2.3"

multiple_body=$(build_body \
  "Reviewer targets: v11.12.13, then 127.0.0.1, then V14.15.16." "" \
  "Added support for 17.18.19." \
  "Fixed a check." \
  "Changed a check." \
  "Remove v20.21.22 from the reviewer appendix.")
assert_rejects_versions \
  "all version occurrences are reported in one validation run" \
  "$multiple_body" \
  "v11.12.13" "V14.15.16" "17.18.19" "v20.21.22"

heading_body=${review_body/Release Notes/Release Notes v6.7.8}
assert_rejects_version \
  "version in the release heading is rejected" \
  "$heading_body" \
  "v6.7.8"

assert_rejects_version \
  "version with a prerelease suffix is rejected" \
  "$(build_body "Reviewer target: 8.9.10-rc.1." "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "8.9.10"

assert_rejects_version \
  "version followed by Markdown punctuation is rejected" \
  "$(build_body "Reviewer target: **v9.10.11.**" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "v9.10.11"

assert_rejects_version \
  "version embedded in an artifact name is rejected" \
  "$(build_body "Reviewer artifact: project-v10.11.12.tar.gz" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "v10.11.12"

assert_rejects_version \
  "version beside an underscore is rejected" \
  "$(build_body "Reviewer artifact: project_v10.11.12.tar.gz" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "v10.11.12"

assert_rejects_version \
  "unprefixed version adjoining text is rejected" \
  "$(build_body "Reviewer artifact: project10.11.12rc1" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "10.11.12"

assert_rejects_version \
  "version after a dotted name is rejected" \
  "$(build_body "Reviewer artifact: project.10.11.12.tar.gz" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "10.11.12"

assert_rejects_version \
  "prefixed version with a fourth component is rejected" \
  "$(build_body "Reviewer target: v10.11.12.13" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "v10.11.12.13"

assert_rejects_version \
  "unprefixed version with five components is rejected" \
  "$(build_body "Reviewer target: 10.11.12.13.14" "" "Added a check." "Fixed a check." "Changed a check." "")" \
  "10.11.12.13.14"

assert_rejects_version \
  "version in What's New is rejected" \
  "$(build_body "" "" "Added support in v2.3.4." "Fixed a check." "Changed a check." "")" \
  "v2.3.4"

assert_rejects_version \
  "version in What's Fixed is rejected" \
  "$(build_body "" "" "Added a check." "Fixed behavior from 3.4.5." "Changed a check." "")" \
  "3.4.5"

assert_rejects_version \
  "version in What's Changed is rejected" \
  "$(build_body "" "" "Added a check." "Fixed a check." "Changed behavior for v4.5.6." "")" \
  "v4.5.6"

assert_rejects_version \
  "version after the release section is rejected" \
  "$(build_body "" "" "Added a check." "Fixed a check." "Changed a check." "Reviewer target: 5.6.7.")" \
  "5.6.7"

assert_rejects_version \
  "version beneath the release heading is rejected with placement errors preserved" \
  "$(build_body "" "v7.8.9" "Added a check." "Fixed a check." "Changed a check." "")" \
  "v7.8.9" \
  "nothing may sit between '## Release Notes'"

printf '%d release-note validator tests passed\n' "$passed"
