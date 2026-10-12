#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
import_script="$script_dir/import-release-images.sh"
suite_tmp=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-import-tests.XXXXXX")
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
  grep -F -- "$2" "$1" >/dev/null || fail "$1 does not contain: $2"
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

component_for() {
  case "$1" in
    *cma-api*) printf 'api\n' ;;
    *cma-web*) printf 'web\n' ;;
    *cma-runtime*) printf 'runtime\n' ;;
    *cma-cmaeg*) printf 'cmaeg\n' ;;
    *cma-image-manager*) printf 'image-manager\n' ;;
    *) printf 'cannot identify component: %s\n' "$1" >&2; exit 71 ;;
  esac
}

kind_for() {
  case "$1" in
    *:"$MOCK_VERSION") printf 'version\n' ;;
    *:"$MOCK_REVISION") printf 'revision\n' ;;
    *) printf 'cannot identify tag: %s\n' "$1" >&2; exit 72 ;;
  esac
}

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
    component=$(component_for "$target")
    if [ "$config" -eq 1 ]; then
      case "$target" in
        docker://*)
          kind=$(kind_for "$target")
          [ "${MOCK_FAIL_DEST_CONFIG:-}" != "$component:$kind:$architecture" ] \
            || exit 80
          ;;
      esac
      cat "$MOCK_FIXTURES/$component.$architecture.config"
    else
      case "$target" in
        oci-archive:*) cat "$MOCK_FIXTURES/$component.raw" ;;
        docker://*)
          kind=$(kind_for "$target")
          state="$MOCK_STATE/$component.$kind.raw"
          [ -f "$state" ] || exit 73
          cat "$state"
          ;;
        *) exit 74 ;;
      esac
    fi
    ;;
  list-tags)
    target=
    for argument in "$@"; do target=$argument; done
    component=$(component_for "$target")
    if [ "${MOCK_LIST_ERROR_COMPONENT:-}" = "$component" ]; then
      printf '%s\n' "${MOCK_LIST_ERROR_TEXT:-transport failure}" >&2
      exit 75
    fi
    separator=
    printf '{"Repository":"cma-%s","Tags":[' "$component"
    if [ -f "$MOCK_STATE/$component.version.raw" ]; then
      printf '"%s"' "$MOCK_VERSION"
      separator=,
    fi
    if [ -f "$MOCK_STATE/$component.revision.raw" ]; then
      printf '%s"%s"' "$separator" "$MOCK_REVISION"
    fi
    printf ']}\n'
    ;;
  copy)
    source=
    destination=
    for argument in "$@"; do
      source=$destination
      destination=$argument
    done
    case "$source" in oci-archive:*) ;; *) exit 76 ;; esac
    case "$destination" in docker://*) ;; *) exit 77 ;; esac
    component=$(component_for "$destination")
    kind=$(kind_for "$destination")
    cp "$MOCK_FIXTURES/$component.raw" "$MOCK_STATE/$component.$kind.raw"
    if [ "${MOCK_FAIL_AFTER_COPY:-}" = "$component:$kind" ]; then
      exit 78
    fi
    ;;
  *) printf 'unexpected Skopeo command: %s\n' "$command" >&2; exit 79 ;;
esac
MOCK
chmod +x "$mock_skopeo"

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$suite_tmp/case-$case_number"
  transfer="$case_dir/transfer"
  fixtures="$case_dir/fixtures"
  state="$case_dir/state"
  log="$case_dir/skopeo.log"
  authfile="$case_dir/auth.json"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"
  version=v1.2.3
  full_revision=abcdef0123456789abcdef0123456789abcdef01
  revision=abcdef0
  destination=registry.example.com/project/images
  list_error_component=
  list_error_text=
  fail_after_copy=
  fail_dest_config=
  mkdir -p "$transfer" "$fixtures" "$state"
  : > "$log"
  printf '%s' 'DO-NOT-LEAK-THIS-CREDENTIAL' > "$authfile"

  cat > "$fixtures/api.raw" <<'JSON'
{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size":1},"layers":[]}
JSON
  cat > "$fixtures/web.raw" <<'JSON'
{"schemaVersion":2,"mediaType":"application/vnd.oci.image.index.v1+json","manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","size":1,"platform":{"os":"linux","architecture":"amd64"}},{"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","size":1,"platform":{"os":"linux","architecture":"arm64"}}]}
JSON
  cat > "$fixtures/runtime.raw" <<'JSON'
{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd","size":1},"layers":[]}
JSON

  cp "$fixtures/api.raw" "$fixtures/cmaeg.raw"
  cp "$fixtures/api.raw" "$fixtures/image-manager.raw"
  for component in api web runtime cmaeg image-manager; do
    printf '{"os":"linux","architecture":"amd64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}\n' \
      "$version" "$full_revision" > "$fixtures/$component.amd64.config"
  done
  printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}\n' \
    "$version" "$full_revision" > "$fixtures/web.arm64.config"

  for component in api web runtime cmaeg image-manager; do
    base="cma-${component}-${version}-${revision}"
    mkdir -p "$transfer/$component"
    archive="$transfer/$component/$base.oci.tar"
    metadata="$transfer/$component/$base.json"
    printf 'archive bytes for %s\n' "$component" > "$archive"
    source_digest=$(sha256_file "$fixtures/$component.raw")
    archive_digest=$(sha256_file "$archive")
    if [ "$component" = web ]; then
      platforms='["linux/amd64","linux/arm64"]'
    else
      platforms='["linux/amd64"]'
    fi
    cat > "$metadata" <<JSON
{
  "schemaVersion": 1,
  "component": "$component",
  "version": "$version",
  "revision": "$revision",
  "fullRevision": "$full_revision",
  "sourceRepository": "ghcr.io/chronoaiproject/cma-$component",
  "versionReference": "ghcr.io/chronoaiproject/cma-$component:$version",
  "revisionReference": "ghcr.io/chronoaiproject/cma-$component:$revision",
  "sourceDigest": "$source_digest",
  "sourceReference": "ghcr.io/chronoaiproject/cma-$component@$source_digest",
  "archive": "$base.oci.tar",
  "archiveReference": "$version",
  "archiveSha256": "$archive_digest",
  "platforms": $platforms
}
JSON
  done
}

set_equal() {
  cp "$fixtures/$1.raw" "$state/$1.$2.raw"
}

run_import() {
  env \
    SKOPEO_COMMAND="$mock_skopeo" \
    SKOPEO_AUTHFILE="$authfile" \
    MOCK_LOG="$log" \
    MOCK_FIXTURES="$fixtures" \
    MOCK_STATE="$state" \
    MOCK_VERSION="$version" \
    MOCK_REVISION="$revision" \
    MOCK_LIST_ERROR_COMPONENT="$list_error_component" \
    MOCK_LIST_ERROR_TEXT="$list_error_text" \
    MOCK_FAIL_AFTER_COPY="$fail_after_copy" \
    MOCK_FAIL_DEST_CONFIG="$fail_dest_config" \
    "$import_script" "$transfer" "$version" "$full_revision" "$destination" \
    > "$stdout" 2> "$stderr"
}

assert_all_equal() {
  for component in api web runtime cmaeg image-manager; do
    for kind in version revision; do
      cmp -s "$fixtures/$component.raw" "$state/$component.$kind.raw" \
        || fail "$component $kind destination is not exact"
    done
  done
}

new_case
run_import || fail "all-absent import failed"
[ "$(grep -c '^<list-tags>' "$log")" -eq 5 ] || fail "preflight did not list all repositories"
[ "$(grep -c '^<copy>' "$log")" -eq 10 ] || fail "all-absent import did not copy ten tags"
third_list=$(grep -n '^<list-tags>' "$log" | tail -1 | cut -d: -f1)
first_copy=$(grep -n '^<copy>' "$log" | head -1 | cut -d: -f1)
[ "$third_list" -lt "$first_copy" ] || fail "copy began before the complete destination preflight"
assert_contains "$log" '<copy><--all><--preserve-digests><--authfile>'
[ "$(grep -c '<inspect><--config>' "$log")" -eq 18 ] \
  || fail "archive and copied tags did not expose all expected platform configs"
assert_all_equal
if grep -F 'DO-NOT-LEAK-THIS-CREDENTIAL' "$log" "$stdout" "$stderr" >/dev/null; then
  fail "credential contents escaped into importer output"
fi
pass "ten absent tags are preflighted, copied exactly, and credential-safe"

new_case
for component in api web runtime cmaeg image-manager; do
  set_equal "$component" version
  set_equal "$component" revision
done
run_import || fail "all-equal retry failed"
[ "$(grep -c '^<copy>' "$log" || true)" -eq 0 ] || fail "equal destination was copied again"
[ "$(grep -c '^Keeping ' "$stdout")" -eq 10 ] || fail "equal tags were not reported as retained"
pass "ten equal tags are idempotently skipped"

new_case
set_equal api version
set_equal web revision
run_import || fail "partially completed import failed"
[ "$(grep -c '^<copy>' "$log")" -eq 8 ] || fail "partial retry did not copy only absent tags"
assert_all_equal
pass "partially completed import copies only missing aliases"

new_case
printf '%s' '{"different":true}' > "$state/runtime.revision.raw"
if run_import; then fail "mismatched destination tag was overwritten"; fi
[ "$(grep -c '^<copy>' "$log" || true)" -eq 0 ] || fail "mismatch was found after mutation began"
assert_contains "$stderr" 'destination tag has a different digest'
pass "one mismatch aborts the ten-tag import before mutation"

new_case
fail_after_copy=api:version
if run_import; then fail "injected post-copy failure was ignored"; fi
[ -f "$state/api.version.raw" ] || fail "partial copy was not recorded by mock"
fail_after_copy=
run_import || fail "retry after partial copy failed"
[ "$(grep -c '^<copy>' "$log")" -eq 10 ] \
  || fail "retry did not skip the tag completed by the failed attempt"
assert_all_equal
pass "retry resumes after an ambiguous post-copy failure"

new_case
list_error_component=api
list_error_text='TLS handshake timeout'
if run_import; then fail "tag-list transport failure was treated as absence"; fi
[ "$(grep -c '^<copy>' "$log" || true)" -eq 0 ] || fail "transport failure allowed mutation"
assert_contains "$stderr" 'cannot determine existing tags'
pass "unknown destination inspection failures abort closed"

new_case
list_error_component=api
list_error_text='name unknown: repository name not known to registry'
run_import || fail "known absent repository was not importable"
assert_all_equal
pass "a registry-confirmed absent repository is treated as ten absent tags"

new_case
printf 'tampered\n' >> "$transfer/web/cma-web-v1.2.3-abcdef0.oci.tar"
if run_import; then fail "tampered archive was accepted"; fi
[ "$(grep -c '^<list-tags>' "$log" || true)" -eq 0 ] || fail "metadata failure reached destination preflight"
[ "$(grep -c '^<copy>' "$log" || true)" -eq 0 ] || fail "metadata failure reached mutation"
assert_contains "$stderr" 'archive checksum mismatch'
pass "archive integrity failure stops before destination access"

new_case
archive="$transfer/api/cma-api-v1.2.3-abcdef0.oci.tar"
rm "$archive"
mkdir "$archive"
if run_import; then fail "archive directory was accepted as a regular file"; fi
[ ! -s "$log" ] || fail "archive directory invoked Skopeo"
assert_contains "$stderr" 'missing non-empty regular archive'
pass "archive directories are rejected before Skopeo access"

new_case
archive="$transfer/api/cma-api-v1.2.3-abcdef0.oci.tar"
rm "$archive"
mkfifo "$archive"
if run_import; then fail "archive FIFO was accepted as a regular file"; fi
[ ! -s "$log" ] || fail "archive FIFO invoked Skopeo"
assert_contains "$stderr" 'missing non-empty regular archive'
pass "archive special files are rejected before Skopeo access"

new_case
printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"wrong"}}}\n' \
  "$version" > "$fixtures/web.arm64.config"
if run_import; then fail "web archive with incorrect child labels was accepted"; fi
[ "$(grep -c '^<list-tags>' "$log" || true)" -eq 0 ] \
  || fail "invalid web child config reached destination preflight"
[ "$(grep -c '^<copy>' "$log" || true)" -eq 0 ] \
  || fail "invalid web child config reached mutation"
assert_contains "$stderr" 'web archive linux/arm64 has incorrect platform or release labels'
pass "web child configs and labels are verified before destination access"

new_case
printf '{"os":"linux","architecture":"arm64","config":{"Labels":{"org.opencontainers.image.version":"%s","org.opencontainers.image.revision":"%s"}}}\n' \
  "$version" "$full_revision" > "$fixtures/api.amd64.config"
if run_import; then fail "arm64 API archive was accepted"; fi
[ "$(grep -c '^<list-tags>' "$log" || true)" -eq 0 ] \
  || fail "invalid API config reached destination preflight"
assert_contains "$stderr" 'api archive linux/amd64 has incorrect platform or release labels'
pass "single-image archive OS and architecture are verified from config"

new_case
set_equal web version
fail_dest_config=web:version:arm64
if run_import; then fail "an equal root with a missing web child config was accepted"; fi
[ "$(grep -c '^<copy>' "$log" || true)" -eq 0 ] \
  || fail "broken existing destination content allowed mutation"
assert_contains "$stderr" 'could not inspect destination'
pass "every existing destination child config is readable before mutation"

new_case
version=$(printf '../../escape\nv1.2.3')
if run_import; then fail "newline path traversal in the version was accepted"; fi
[ ! -s "$log" ] || fail "newline path traversal invoked Skopeo"
pass "newline path traversal cannot bypass importer version validation"

new_case
destination=$(printf 'registry.invalid/escape\nregistry.example.com/project/images')
if run_import; then fail "newline injection in the destination was accepted"; fi
[ ! -s "$log" ] || fail "newline destination invoked Skopeo"
pass "newline injection cannot bypass destination validation"

printf '1..%d\n' "$tests"
