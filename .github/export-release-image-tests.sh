#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
export_script="$script_dir/export-release-image.sh"
export_invocation=("$export_script")
if [ -n "${EXPORT_RELEASE_IMAGE_SHELL:-}" ]; then
  export_invocation=("$EXPORT_RELEASE_IMAGE_SHELL" "$export_script")
fi
suite_tmp=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-export-tests.XXXXXX")
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

assert_empty_dir() {
  directory=$1
  [ -z "$(find "$directory" -mindepth 1 -maxdepth 1 -print -quit)" ] \
    || fail "$directory contains an unexpected output"
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
  inspect)
    target=
    architecture=
    config=0
    previous=
    for argument in "$@"; do
      if [ "$previous" = --override-arch ]; then architecture=$argument; fi
      [ "$argument" != --config ] || config=1
      previous=$argument
      target=$argument
    done
    if [ "$config" -eq 1 ]; then
      case "$target:$architecture" in
        docker://*@sha256:*:amd64) printf '%s' "$MOCK_SOURCE_AMD64_CONFIG" ;;
        docker://*@sha256:*:arm64) printf '%s' "$MOCK_SOURCE_ARM64_CONFIG" ;;
        oci-archive:*:amd64) printf '%s' "$MOCK_ARCHIVE_AMD64_CONFIG" ;;
        oci-archive:*:arm64) printf '%s' "$MOCK_ARCHIVE_ARM64_CONFIG" ;;
        *) printf 'unexpected config target: %s (%s)\n' "$target" "$architecture" >&2; exit 41 ;;
      esac
    else
      case "$target" in
        "$MOCK_VERSION_REF") printf '%s' "$MOCK_VERSION_MANIFEST" ;;
        "$MOCK_REVISION_REF") printf '%s' "$MOCK_REVISION_MANIFEST" ;;
        oci-archive:*) printf '%s' "$MOCK_ARCHIVE_MANIFEST" ;;
        *) printf 'unexpected inspect target: %s\n' "$target" >&2; exit 41 ;;
      esac
    fi
    ;;
  copy)
    [ "${MOCK_FORBID_COPY:-0}" -ne 1 ] || exit 42
    source=
    destination=
    for argument in "$@"; do
      source=$destination
      destination=$argument
    done
    case "$source" in docker://*@sha256:*) ;; *) exit 43 ;; esac
    case "$destination" in oci-archive:*:*) ;; *) exit 44 ;; esac
    archive_spec=${destination#oci-archive:}
    archive_path=${archive_spec%%:*}
    printf '%s' "${MOCK_ARCHIVE_BYTES:-archive bytes}" > "$archive_path"
    ;;
  *) printf 'unexpected Skopeo command: %s\n' "$command" >&2; exit 45 ;;
esac
MOCK
chmod +x "$mock_skopeo"

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$suite_tmp/case-$case_number"
  output_dir="$case_dir/output"
  log="$case_dir/skopeo.log"
  authfile="$case_dir/auth.json"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"
  mkdir -p "$output_dir"
  : > "$log"
  printf '%s' 'TOP-SECRET-CREDENTIAL' > "$authfile"

  component=web
  version=v1.2.3
  full_revision=abcdef0123456789abcdef0123456789abcdef01
  revision=abcdef0
  source=ghcr.io/chronoai/cma-web
  version_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:1111","platform":{"os":"linux","architecture":"amd64"}},{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:2222","platform":{"os":"linux","architecture":"arm64"}}]}'
  revision_manifest=$version_manifest
  archive_manifest=$version_manifest
  source_amd64_config=$(printf '{"os":"linux","architecture":"amd64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}' "$version" "$full_revision")
  source_arm64_config=$(printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}' "$version" "$full_revision")
  archive_amd64_config=$source_amd64_config
  archive_arm64_config=$source_arm64_config
  archive_bytes='mock OCI archive bytes'
  forbid_copy=0
  version_ref="docker://${source}:${version}"
  revision_ref="docker://${source}:${revision}"
}

run_export() {
  env \
    SKOPEO_COMMAND="$mock_skopeo" \
    SKOPEO_AUTHFILE="$authfile" \
    MOCK_LOG="$log" \
    MOCK_VERSION_REF="$version_ref" \
    MOCK_REVISION_REF="$revision_ref" \
    MOCK_VERSION_MANIFEST="$version_manifest" \
    MOCK_REVISION_MANIFEST="$revision_manifest" \
    MOCK_ARCHIVE_MANIFEST="$archive_manifest" \
    MOCK_SOURCE_AMD64_CONFIG="$source_amd64_config" \
    MOCK_SOURCE_ARM64_CONFIG="$source_arm64_config" \
    MOCK_ARCHIVE_AMD64_CONFIG="$archive_amd64_config" \
    MOCK_ARCHIVE_ARM64_CONFIG="$archive_arm64_config" \
    MOCK_ARCHIVE_BYTES="$archive_bytes" \
    MOCK_FORBID_COPY="${forbid_copy:-0}" \
    "${export_invocation[@]}" \
    "$component" "$version" "$full_revision" "$source" "$output_dir" \
    > "$stdout" 2> "$stderr"
}

new_case
run_export || fail "valid multi-platform export failed"
archive="cma-web-v1.2.3-abcdef0.oci.tar"
metadata="cma-web-v1.2.3-abcdef0.json"
[ -s "$output_dir/$archive" ] || fail "archive was not created"
[ -s "$output_dir/$metadata" ] || fail "metadata was not created"
[ "$(find "$output_dir" -mindepth 1 -maxdepth 1 -type f | wc -l | tr -d ' ')" -eq 2 ] \
  || fail "successful export did not produce exactly two files"
printf '%s' "$version_manifest" > "$case_dir/source.raw"
source_digest=$(sha256_file "$case_dir/source.raw")
archive_digest=$(sha256_file "$output_dir/$archive")
assert_contains "$output_dir/$metadata" '"schemaVersion": 1'
assert_contains "$output_dir/$metadata" '"component": "web"'
assert_contains "$output_dir/$metadata" '"version": "v1.2.3"'
assert_contains "$output_dir/$metadata" '"revision": "abcdef0"'
assert_contains "$output_dir/$metadata" '"fullRevision": "abcdef0123456789abcdef0123456789abcdef01"'
assert_contains "$output_dir/$metadata" "\"sourceRepository\": \"$source\""
assert_contains "$output_dir/$metadata" "\"versionReference\": \"${source}:${version}\""
assert_contains "$output_dir/$metadata" "\"revisionReference\": \"${source}:${revision}\""
assert_contains "$output_dir/$metadata" "\"sourceDigest\": \"$source_digest\""
assert_contains "$output_dir/$metadata" "\"sourceReference\": \"${source}@${source_digest}\""
assert_contains "$output_dir/$metadata" "\"archive\": \"$archive\""
assert_contains "$output_dir/$metadata" "\"archiveReference\": \"$version\""
assert_contains "$output_dir/$metadata" "\"archiveSha256\": \"$archive_digest\""
assert_contains "$output_dir/$metadata" '"platforms": ["linux/amd64","linux/arm64"]'
assert_contains "$log" '<inspect><--raw><--authfile>'
assert_contains "$log" "<docker://${source}:${version}>"
assert_contains "$log" "<docker://${source}:${revision}>"
assert_contains "$log" "<copy><--all><--preserve-digests><--authfile><$authfile><docker://${source}@${source_digest}><oci-archive:.cma-image-export."
assert_contains "$log" "/${archive}:${version}>"
assert_contains "$log" '<inspect><--config><--override-os><linux><--override-arch><amd64>'
assert_contains "$log" '<inspect><--config><--override-os><linux><--override-arch><arm64>'
if grep -F 'TOP-SECRET-CREDENTIAL' "$log" "$output_dir"/* >/dev/null; then
  fail "credential contents escaped into logs or outputs"
fi
if grep -E '<(build|push)>' "$log" >/dev/null; then
  fail "export invoked an image build or push command"
fi
pass "equal raw tags copy by digest with all manifests and no credential leak"

new_case
revision_manifest='{"schemaVersion":2,"mediaType":"different"}'
forbid_copy=1
if run_export; then
  fail "different version and revision manifests were accepted"
fi
assert_empty_dir "$output_dir"
[ "$(grep -c '^<inspect>' "$log")" -eq 2 ] || fail "tag mismatch did not inspect both tags"
if grep -q '^<copy>' "$log"; then fail "tag mismatch reached copy"; fi
pass "tag digest mismatch fails before archive creation"

new_case
archive_manifest='{"schemaVersion":2,"mediaType":"rewritten"}'
if run_export; then
  fail "archive with a different root digest was accepted"
fi
assert_empty_dir "$output_dir"
assert_contains "$stderr" 'OCI archive root digest does not match'
pass "archive root digest mismatch removes partial outputs"

new_case
component=worker
source=ghcr.io/chronoai/cma-worker
if run_export; then fail "invalid component was accepted"; fi
assert_empty_dir "$output_dir"
[ ! -s "$log" ] || fail "invalid component invoked Skopeo"
pass "invalid component is rejected before registry access"

new_case
version=1.2.3
if run_export; then fail "invalid version was accepted"; fi
assert_empty_dir "$output_dir"
[ ! -s "$log" ] || fail "invalid version invoked Skopeo"
pass "invalid version is rejected before registry access"

new_case
full_revision=ABCDEF0123456789abcdef0123456789abcdef01
if run_export; then fail "invalid revision was accepted"; fi
assert_empty_dir "$output_dir"
[ ! -s "$log" ] || fail "invalid revision invoked Skopeo"
pass "invalid revision is rejected before registry access"

new_case
version=$(printf '../../escape\nv1.2.3')
if run_export; then fail "newline path traversal in the version was accepted"; fi
assert_empty_dir "$output_dir"
[ ! -s "$log" ] || fail "newline path traversal invoked Skopeo"
pass "newline path traversal cannot bypass full-value validation"

new_case
source=ghcr.io/chronoai/other-web
if run_export; then fail "mismatched source repository was accepted"; fi
assert_empty_dir "$output_dir"
[ ! -s "$log" ] || fail "invalid source invoked Skopeo"
pass "source repository is bounded to the selected component"

new_case
: > "$output_dir/cma-web-v1.2.3-abcdef0.oci.tar"
if run_export; then fail "existing archive was overwritten"; fi
[ ! -s "$output_dir/cma-web-v1.2.3-abcdef0.oci.tar" ] \
  || fail "existing archive contents changed"
[ ! -s "$log" ] || fail "overwrite refusal invoked Skopeo"
pass "deterministic output paths are never overwritten"

new_case
ln -s "$case_dir/missing-archive-target" \
  "$output_dir/cma-web-v1.2.3-abcdef0.oci.tar"
if run_export; then fail "a dangling archive symlink was followed"; fi
[ -L "$output_dir/cma-web-v1.2.3-abcdef0.oci.tar" ] \
  || fail "dangling archive symlink was changed"
[ ! -s "$log" ] || fail "dangling archive symlink invoked Skopeo"
pass "a dangling archive symlink is refused before registry access"

new_case
ln -s "$case_dir/missing-metadata-target" \
  "$output_dir/cma-web-v1.2.3-abcdef0.json"
if run_export; then fail "a dangling metadata symlink was followed"; fi
[ -L "$output_dir/cma-web-v1.2.3-abcdef0.json" ] \
  || fail "dangling metadata symlink was changed"
[ ! -s "$log" ] || fail "dangling metadata symlink invoked Skopeo"
pass "a dangling metadata symlink is refused before registry access"

new_case
component=api
source=ghcr.io/chronoai/cma-agent-manager
version_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"digest":"sha256:3333"},"layers":[]}'
revision_manifest=$version_manifest
archive_manifest=$version_manifest
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
run_export || fail "valid single-platform export failed"
metadata="cma-api-v1.2.3-abcdef0.json"
assert_contains "$output_dir/$metadata" '"platforms": ["linux/amd64"]'
if grep -F 'linux/arm64' "$output_dir/$metadata" >/dev/null; then
  fail "single-platform metadata claims arm64"
fi
pass "single-platform metadata remains exact"

new_case
version_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"digest":"sha256:1111","platform":{"os":"linux","architecture":"amd64"}}]}'
revision_manifest=$version_manifest
archive_manifest=$version_manifest
forbid_copy=1
if run_export; then fail "web image with a missing platform was accepted"; fi
assert_empty_dir "$output_dir"
if grep -q '^<copy>' "$log"; then fail "invalid web platform shape reached copy"; fi
pass "web platforms are verified from the raw source index"

new_case
source_arm64_config=$(printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"wrong"}}}' "$version")
forbid_copy=1
if run_export; then fail "web child with incorrect labels was accepted"; fi
assert_empty_dir "$output_dir"
if grep -q '^<copy>' "$log"; then fail "invalid web child labels reached copy"; fi
pass "both web child configs and release labels are verified before copy"

new_case
archive_arm64_config=$(printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"wrong"}}}' "$version")
if run_export; then fail "archive child with incorrect labels was accepted"; fi
assert_empty_dir "$output_dir"
assert_contains "$stderr" 'archive linux/arm64 has incorrect platform or release labels'
pass "archive child configs are verified before outputs become visible"

new_case
component=runtime
source=ghcr.io/chronoai/cma-runtime
version_manifest='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"digest":"sha256:4444"},"layers":[]}'
revision_manifest=$version_manifest
archive_manifest=$version_manifest
source_amd64_config=$(printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}' "$version" "$full_revision")
version_ref="docker://${source}:${version}"
revision_ref="docker://${source}:${revision}"
forbid_copy=1
if run_export; then fail "arm64 runtime image was accepted"; fi
assert_empty_dir "$output_dir"
if grep -q '^<copy>' "$log"; then fail "invalid runtime platform reached copy"; fi
pass "single-image platform is verified from the source config"

printf '1..%d\n' "$tests"
