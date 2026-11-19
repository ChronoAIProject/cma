#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
assert_script="$script_dir/assert-release-image-tags-absent.sh"
suite_tmp=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-absence-tests.XXXXXX")
tests=0

cleanup() {
  rm -rf "$suite_tmp"
}
trap cleanup EXIT HUP INT TERM

fail() {
  printf 'not ok - %s\n' "$*" >&2
  exit 1
}

pass() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "$tests" "$1"
}

assert_contains() {
  grep -F -- "$2" "$1" >/dev/null \
    || fail "$1 does not contain: $2"
}

assert_read_only() {
  if grep -E '^<(inspect|copy|build|push)>' "$log" >/dev/null; then
    fail 'the absence assertion performed a mutating or content-inspection operation'
  fi
}

mock_skopeo="$suite_tmp/skopeo"
cat > "$mock_skopeo" <<'MOCK'
#!/bin/sh
set -eu

for argument in "$@"; do
  printf '<%s>' "$argument" >> "$MOCK_LOG"
done
printf '\n' >> "$MOCK_LOG"

[ "$1" = list-tags ] || exit 71
[ "${MOCK_FAIL:-}" != list-tags ] || exit 72
printf '{"Repository":"mock","Tags":%s}\n' "$MOCK_TAGS"
MOCK
chmod 0755 "$mock_skopeo"

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$suite_tmp/case-$case_number"
  log="$case_dir/skopeo.log"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"
  authfile="$case_dir/auth.json"
  component=web
  version=v1.2.3
  full_revision=0123456789abcdef0123456789abcdef01234567
  revision=0123456
  source=ghcr.io/chronoai/cma-web
  tags='[]'
  fail_command=
  mkdir -p "$case_dir"
  : > "$log"
  printf '%s' 'DO-NOT-PRINT-THIS-CREDENTIAL' > "$authfile"
}

run_assertion() {
  env \
    SKOPEO_COMMAND="$mock_skopeo" \
    SKOPEO_AUTHFILE="$authfile" \
    MOCK_LOG="$log" \
    MOCK_TAGS="$tags" \
    MOCK_FAIL="$fail_command" \
    "$assert_script" "$component" "$version" "$full_revision" "$source" \
    > "$stdout" 2> "$stderr"
}

new_case
run_assertion || fail 'two absent tags were rejected'
[ "$(wc -l < "$log" | tr -d ' ')" -eq 1 ] \
  || fail 'the success path did more than one tag listing'
assert_contains "$log" \
  "<list-tags><--authfile><$authfile><docker://$source>"
assert_contains "$stdout" "$source:$version and $source:$revision are still absent"
assert_read_only
if grep -F 'DO-NOT-PRINT-THIS-CREDENTIAL' "$log" "$stdout" "$stderr" >/dev/null; then
  fail 'authentication contents escaped into assertion output'
fi
pass 'both absent tags pass through one credential-safe read-only listing'

new_case
tags="[\"$version\"]"
if run_assertion; then fail 'a newly appeared version tag was accepted'; fi
assert_contains "$stderr" "release image tag appeared after reconciliation: $version"
assert_read_only
pass 'a version tag appearing after reconciliation fails closed'

new_case
tags="[\"$revision\"]"
if run_assertion; then fail 'a newly appeared revision tag was accepted'; fi
assert_contains "$stderr" "release image tag appeared after reconciliation: $revision"
assert_read_only
pass 'a revision tag appearing after reconciliation fails closed'

new_case
tags="[\"$version\",\"$revision\"]"
if run_assertion; then fail 'two newly appeared release tags were accepted'; fi
assert_contains "$stderr" 'release image tag appeared after reconciliation'
assert_read_only
pass 'both tags appearing after reconciliation fail closed without reuse'

new_case
tags='null'
if run_assertion; then fail 'a malformed tag listing was accepted'; fi
assert_contains "$stderr" 'malformed tag listing immediately before publish'
assert_read_only
pass 'a malformed tag listing cannot authorize publish'

new_case
fail_command=list-tags
if run_assertion; then fail 'a tag-list transport failure was accepted'; fi
assert_contains "$stderr" 'could not list the source repository tags immediately before publish'
assert_read_only
pass 'a tag-list failure cannot authorize publish'

new_case
version=$(printf 'v1.2.3\nrelease-injection')
if run_assertion; then fail 'a newline-injected version was accepted'; fi
[ ! -s "$log" ] || fail 'invalid input reached Skopeo'
assert_read_only
pass 'newline injection fails before registry access'

for selected in cmaeg runtime-omarchy; do
  new_case
  component=$selected
  image_name=$(python3 "$script_dir/scripts/deployables.py" "$component")
  source="ghcr.io/chronoai/$image_name"
  run_assertion || fail "$component was rejected before publication"
  assert_read_only
  pass "$component has the same tag-absence gate as the other images"
done

for retired in image-manager runtime-desktop; do
  new_case
  component=$retired
  source="ghcr.io/chronoai/cma-$component"
  if run_assertion; then fail 'retired component passed publication preflight'; fi
  [ ! -s "$log" ] || fail 'retired component reached the registry'
  pass "$component is refused before registry access"
done

printf '1..%d\n' "$tests"
