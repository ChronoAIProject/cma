#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
reconcile_script="$script_dir/reconcile-release-image.sh"
suite_tmp=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-reconcile-tests.XXXXXX")
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
  file=$1
  text=$2
  grep -F -- "$text" "$file" >/dev/null \
    || fail "$file does not contain: $text"
}

assert_no_copy_or_build() {
  if grep -E '^<copy>|<(build|push)>' "$log" >/dev/null; then
    fail "a fail-closed path copied, built, or pushed an image"
  fi
}

assert_no_build_decision() {
  if grep -F 'reuse=false' "$output" >/dev/null; then
    fail "a command or validation failure authorized a rebuild"
  fi
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$1")
  else
    sum=$(shasum -a 256 "$1")
  fi
  printf 'sha256:%s\n' "${sum%% *}"
}

mock_skopeo="$suite_tmp/skopeo"
cat > "$mock_skopeo" <<'MOCK'
#!/bin/sh
set -eu

for argument in "$@"; do
  printf '<%s>' "$argument" >> "$MOCK_LOG"
done
printf '\n' >> "$MOCK_LOG"

command=$1
shift
case "$command" in
  list-tags)
    [ "${MOCK_FAIL:-}" != list-tags ] || exit 41
    printf '{"Repository":"mock","Tags":%s}' "$MOCK_TAGS"
    ;;
  inspect)
    target=
    architecture=
    raw=0
    config=0
    previous=
    for argument in "$@"; do
      if [ "$previous" = --override-arch ]; then
        architecture=$argument
      fi
      case "$argument" in
        --raw) raw=1 ;;
        --config) config=1 ;;
      esac
      previous=$argument
      target=$argument
    done
    if [ "$raw" -eq 1 ]; then
      copied=0
      if [ -s "$MOCK_COPIED" ] && grep -Fx -- "$target" "$MOCK_COPIED" >/dev/null; then
        copied=1
      fi
      if [ "$copied" -eq 1 ]; then
        [ "${MOCK_FAIL:-}" != inspect-repaired ] || exit 42
        printf '%s' "$MOCK_REPAIRED_MANIFEST"
      elif [ "$target" = "$MOCK_VERSION_REF" ]; then
        [ "${MOCK_FAIL:-}" != inspect-version ] || exit 43
        printf '%s' "$MOCK_VERSION_MANIFEST"
      elif [ "$target" = "$MOCK_REVISION_REF" ]; then
        [ "${MOCK_FAIL:-}" != inspect-revision ] || exit 44
        printf '%s' "$MOCK_REVISION_MANIFEST"
      else
        printf 'unexpected raw inspection target: %s\n' "$target" >&2
        exit 45
      fi
    elif [ "$config" -eq 1 ]; then
      [ "${MOCK_FAIL:-}" != "config-$architecture" ] || exit 46
      case "$architecture" in
        amd64) printf '%s' "$MOCK_AMD64_CONFIG" ;;
        arm64) printf '%s' "$MOCK_ARM64_CONFIG" ;;
        *) printf 'unexpected architecture: %s\n' "$architecture" >&2; exit 47 ;;
      esac
    else
      exit 48
    fi
    ;;
  copy)
    [ "${MOCK_FAIL:-}" != copy ] || exit 49
    source=
    destination=
    for argument in "$@"; do
      source=$destination
      destination=$argument
    done
    case "$source" in docker://*@sha256:*) ;; *) exit 50 ;; esac
    case "$destination" in docker://*:*) ;; *) exit 51 ;; esac
    printf '%s\n' "$destination" >> "$MOCK_COPIED"
    ;;
  *)
    printf 'unexpected Skopeo command: %s\n' "$command" >&2
    exit 52
    ;;
esac
MOCK
chmod 0755 "$mock_skopeo"

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$suite_tmp/case-$case_number"
  log="$case_dir/skopeo.log"
  output="$case_dir/github-output"
  copied="$case_dir/copied"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"
  mkdir -p "$case_dir"
  : > "$log"
  : > "$output"
  : > "$copied"

  component=web
  version=v1.2.3
  full_revision=0123456789abcdef0123456789abcdef01234567
  revision=0123456
  source=ghcr.io/chronoai/cma-web
  version_ref="docker://${source}:${version}"
  revision_ref="docker://${source}:${revision}"
  tags="[\"$version\",\"$revision\"]"
  fail_command=
  web_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":"sha256:1111","platform":{"os":"linux","architecture":"amd64"}},{"digest":"sha256:2222","platform":{"os":"linux","architecture":"arm64"}}]}'
  plain_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"digest":"sha256:3333"},"layers":[]}'
  version_manifest=$web_manifest
  revision_manifest=$web_manifest
  repaired_manifest=$web_manifest
  amd64_config=$(printf '{"architecture":"amd64","os":"linux","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}' "$version" "$full_revision")
  arm64_config=$(printf '{"architecture":"arm64","os":"linux","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}' "$version" "$full_revision")
}

run_reconcile() {
  env \
    GITHUB_OUTPUT="$output" \
    SKOPEO_COMMAND="$mock_skopeo" \
    SKOPEO_AUTHFILE="$case_dir/auth.json" \
    MOCK_LOG="$log" \
    MOCK_COPIED="$copied" \
    MOCK_TAGS="$tags" \
    MOCK_FAIL="$fail_command" \
    MOCK_VERSION_REF="$version_ref" \
    MOCK_REVISION_REF="$revision_ref" \
    MOCK_VERSION_MANIFEST="$version_manifest" \
    MOCK_REVISION_MANIFEST="$revision_manifest" \
    MOCK_REPAIRED_MANIFEST="$repaired_manifest" \
    MOCK_AMD64_CONFIG="$amd64_config" \
    MOCK_ARM64_CONFIG="$arm64_config" \
    "$reconcile_script" "$component" "$version" "$full_revision" "$source" \
    > "$stdout" 2> "$stderr"
}

new_case
tags='[]'
run_reconcile || fail "an empty tag pair did not authorize the first build"
assert_contains "$output" 'reuse=false'
assert_contains "$output" 'source_digest='
assert_contains "$output" 'repaired_tag=none'
assert_contains "$output" "revision=$revision"
[ "$(wc -l < "$log" | tr -d ' ')" -eq 1 ] || fail "empty tags performed more than listing"
assert_no_copy_or_build
pass "neither tag exists and only that state authorizes a build"

new_case
run_reconcile || fail "equal existing tags were not reused"
printf '%s' "$web_manifest" > "$case_dir/raw"
digest=$(sha256_file "$case_dir/raw")
assert_contains "$output" 'reuse=true'
assert_contains "$output" "source_digest=$digest"
assert_contains "$output" 'repaired_tag=none'
[ "$(grep -c '^<inspect><--config>' "$log")" -eq 2 ] \
  || fail "web did not validate both platform configurations"
assert_no_copy_or_build
pass "equal existing tags and exact web platforms are reused"

new_case
revision_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[]}'
if run_reconcile; then fail "different raw tag digests were accepted"; fi
assert_no_build_decision
assert_no_copy_or_build
pass "different existing tag digests fail closed"

new_case
component=api
source=ghcr.io/chronoai/cma-api
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
tags="[\"$version\"]"
version_manifest=$plain_manifest
revision_manifest=$plain_manifest
repaired_manifest=$plain_manifest
run_reconcile || fail "a missing revision alias was not repaired"
printf '%s' "$plain_manifest" > "$case_dir/raw"
digest=$(sha256_file "$case_dir/raw")
assert_contains "$output" 'reuse=true'
assert_contains "$output" 'repaired_tag=revision'
assert_contains "$log" "<copy><--all><--preserve-digests><--authfile><$case_dir/auth.json><docker://${source}@${digest}><docker://${source}:${revision}>"
assert_contains "$log" "<inspect><--raw><--authfile><$case_dir/auth.json><docker://${source}:${revision}>"
pass "a validated version tag repairs only the revision alias by digest"

new_case
component=runtime
source=ghcr.io/chronoai/cma-runtime
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
tags="[\"$revision\"]"
version_manifest=$plain_manifest
revision_manifest=$plain_manifest
repaired_manifest=$plain_manifest
run_reconcile || fail "a missing version alias was not repaired"
printf '%s' "$plain_manifest" > "$case_dir/raw"
digest=$(sha256_file "$case_dir/raw")
assert_contains "$output" 'reuse=true'
assert_contains "$output" 'repaired_tag=version'
assert_contains "$log" "<copy><--all><--preserve-digests><--authfile><$case_dir/auth.json><docker://${source}@${digest}><docker://${source}:${version}>"
pass "a validated revision tag repairs only the version alias by digest"

new_case
component=api
source=ghcr.io/chronoai/cma-api
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
version_manifest=$plain_manifest
revision_manifest=$plain_manifest
amd64_config='{"architecture":"amd64","os":"linux","config":{"Labels":{"org.opencontainers.image.version":"v9.9.9","org.opencontainers.image.revision":"wrong"}}}'
if run_reconcile; then fail "incorrect release labels were accepted"; fi
assert_no_build_decision
assert_no_copy_or_build
pass "label mismatch fails closed without rewriting either tag"

new_case
version_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":"sha256:1111","platform":{"os":"linux","architecture":"amd64"}}]}'
revision_manifest=$version_manifest
if run_reconcile; then fail "an incomplete web platform index was accepted"; fi
assert_no_build_decision
assert_no_copy_or_build
pass "web platform mismatch fails closed"

new_case
component=api
source=ghcr.io/chronoai/cma-api
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
version_manifest=$web_manifest
revision_manifest=$web_manifest
if run_reconcile; then fail "an API root index was accepted"; fi
assert_no_build_decision
assert_no_copy_or_build
pass "API and runtime require a plain linux/amd64 manifest"

new_case
fail_command=list-tags
if run_reconcile; then fail "a failed tag listing authorized a build"; fi
assert_no_build_decision
assert_no_copy_or_build
pass "tag-listing failure has no rebuild fallback"

new_case
fail_command=inspect-version
if run_reconcile; then fail "a failed tag inspection was accepted"; fi
assert_no_build_decision
assert_no_copy_or_build
pass "inspection failure has no rebuild fallback"

new_case
component=api
source=ghcr.io/chronoai/cma-api
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
tags="[\"$version\"]"
version_manifest=$plain_manifest
revision_manifest=$plain_manifest
repaired_manifest=$plain_manifest
fail_command=copy
if run_reconcile; then fail "a failed alias repair was accepted"; fi
assert_no_build_decision
assert_contains "$log" '<inspect><--config>'
if grep -E '<(build|push)>' "$log" >/dev/null; then fail "repair failure rebuilt an image"; fi
pass "repair failure occurs only after validation and never rebuilds"

new_case
component=api
source=ghcr.io/chronoai/cma-api
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
tags="[\"$version\"]"
version_manifest=$plain_manifest
revision_manifest=$plain_manifest
repaired_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"digest":"sha256:different"},"layers":[]}'
if run_reconcile; then fail "a digest-changing alias repair was accepted"; fi
assert_no_build_decision
if grep -E '<(build|push)>' "$log" >/dev/null; then fail "repair verification rebuilt an image"; fi
pass "repaired alias digest is verified before reuse"

new_case
version=$(printf '../../escape\nv1.2.3')
if run_reconcile; then fail "newline path traversal in the version was accepted"; fi
assert_no_build_decision
assert_no_copy_or_build
[ ! -s "$log" ] || fail "newline path traversal invoked Skopeo"
pass "newline path traversal cannot authorize image reconciliation"

printf '1..%d\n' "$tests"
