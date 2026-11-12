#!/usr/bin/env bash

set -euo pipefail

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: %s TRANSFER_DIR VERSION FULL_REVISION DESTINATION_PREFIX\n' "$0" >&2
  exit 2
}

is_match() {
  local value
  value=$1
  [ "$(printf '%s' "$value" | tr -d '\r\n')" = "$value" ] || return 1
  printf '%s' "$value" | LC_ALL=C grep -Eq "$2"
}

sha256_file() {
  local path sum
  path=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$path") || return 1
  elif command -v shasum >/dev/null 2>&1; then
    sum=$(shasum -a 256 "$path") || return 1
  else
    die "sha256sum or shasum is required"
  fi
  sum=${sum%% *}
  is_match "$sum" '^[0-9a-f]{64}$' \
    || die "SHA-256 tool returned an invalid digest for $path"
  printf 'sha256:%s\n' "$sum"
}

skopeo_inspect_raw() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" inspect --raw --authfile "$skopeo_authfile" "$1"
  else
    "$skopeo_command" inspect --raw "$1"
  fi
}

skopeo_inspect_config() {
  local architecture reference
  architecture=$1
  reference=$2
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" inspect --config \
      --override-os linux --override-arch "$architecture" \
      --authfile "$skopeo_authfile" "$reference"
  else
    "$skopeo_command" inspect --config \
      --override-os linux --override-arch "$architecture" "$reference"
  fi
}

skopeo_list_tags() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" list-tags --authfile "$skopeo_authfile" "$1"
  else
    "$skopeo_command" list-tags "$1"
  fi
}

skopeo_copy_all() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" copy --all --preserve-digests \
      --authfile "$skopeo_authfile" "$1" "$2"
  else
    "$skopeo_command" copy --all --preserve-digests "$1" "$2"
  fi
}

verify_root_shape() {
  local component raw
  component=$1
  raw=$2
  if [ "$component" = web ]; then
    jq -e '
      .mediaType == "application/vnd.oci.image.index.v1+json" and
      (.manifests | type == "array" and length == 2) and
      all(.manifests[];
        .mediaType == "application/vnd.oci.image.manifest.v1+json") and
      ([.manifests[].platform | (.os + "/" + .architecture)] | sort ==
       ["linux/amd64", "linux/arm64"])
    ' "$raw" >/dev/null \
      || die "web archive is not the exact OCI amd64+arm64 image index"
  else
    jq -e '
      .mediaType == "application/vnd.oci.image.manifest.v1+json" and
      (has("manifests") | not)
    ' "$raw" >/dev/null \
      || die "$component archive root is not a single OCI image manifest"
  fi
}

verify_config() {
  local architecture reference destination description
  architecture=$1
  reference=$2
  destination=$3
  description=$4
  if ! skopeo_inspect_config "$architecture" "$reference" > "$destination"; then
    die "could not inspect $description linux/$architecture configuration"
  fi
  [ -s "$destination" ] \
    || die "$description linux/$architecture returned an empty configuration"
  jq -e \
    --arg architecture "$architecture" \
    --arg version "$version" \
    --arg revision "$full_revision" '
      .os == "linux" and .architecture == $architecture and
      .config.Labels["org.opencontainers.image.version"] == $version and
      .config.Labels["org.opencontainers.image.revision"] == $revision
    ' "$destination" >/dev/null \
    || die "$description linux/$architecture has incorrect platform or release labels"
  jq -r '(.os + "/" + .architecture)' "$destination"
}

verify_destination_configs() {
  local component reference prefix description
  component=$1
  reference=$2
  prefix=$3
  description=$4
  verify_config amd64 "$reference" "${prefix}-amd64.config" "$description" \
    >/dev/null
  if [ "$component" = web ]; then
    verify_config arm64 "$reference" "${prefix}-arm64.config" "$description" \
      >/dev/null
  fi
}

[ "$#" -eq 4 ] || usage

transfer_dir=$1
version=$2
full_revision=$3
destination_prefix=$4
skopeo_command=${SKOPEO_COMMAND:-skopeo}
skopeo_authfile=${SKOPEO_AUTHFILE:-}

is_match "$version" '^v[0-9]+\.[0-9]+\.[0-9]+$' \
  || die "version must have the form vMAJOR.MINOR.PATCH"
is_match "$full_revision" '^[0-9a-f]{40}$' \
  || die "revision must be a full 40-character lowercase commit SHA"
is_match "$destination_prefix" \
  '^[a-z0-9][a-z0-9.-]*(:[0-9]+)?(/[a-z0-9]+([._-][a-z0-9]+)*)+$' \
  || die "destination prefix must be a lowercase registry and repository path"
[ -d "$transfer_dir" ] || die "transfer directory does not exist: $transfer_dir"
[ ! -L "$transfer_dir" ] || die "transfer directory must not be a symbolic link"
command -v jq >/dev/null 2>&1 || die "jq is required"
command -v "$skopeo_command" >/dev/null 2>&1 \
  || die "Skopeo command is not executable: $skopeo_command"

revision=${full_revision:0:7}

transfer_dir=$(CDPATH='' cd -- "$transfer_dir" && pwd -P) \
  || die "cannot resolve transfer directory: $transfer_dir"
cd "$transfer_dir"

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-import.XXXXXX") \
  || die "cannot create temporary directory"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT HUP INT TERM

# Validate every archive and its authoritative root before looking at or
# changing the destination. The copy phase consumes only these checked files.
for component in api web runtime cmaeg; do
  base="cma-${component}-${version}-${revision}"
  metadata="${component}/${base}.json"
  archive="${component}/${base}.oci.tar"
  [ -d "$component" ] && [ ! -L "$component" ] \
    || die "missing real component directory: $component"
  [ -f "$metadata" ] && [ ! -L "$metadata" ] \
    || die "missing regular metadata file: $metadata"
  [ -f "$archive" ] && [ -s "$archive" ] && [ ! -L "$archive" ] \
    || die "missing non-empty regular archive: $archive"

  source_repository=$(jq -er '.sourceRepository' "$metadata") \
    || die "metadata has no source repository: $metadata"
  is_match "$source_repository" \
    "^ghcr\\.io/[a-z0-9]+([._-][a-z0-9]+)*/cma-${component}$" \
    || die "metadata source does not match component $component"
  if [ "$component" = web ]; then
    expected_platforms='["linux/amd64", "linux/arm64"]'
  else
    expected_platforms='["linux/amd64"]'
  fi

  jq -e --arg component "$component" --arg version "$version" \
    --arg revision "$revision" --arg full_revision "$full_revision" \
    --arg repository "$source_repository" \
    --arg archive "${base}.oci.tar" --argjson platforms "$expected_platforms" '
      .schemaVersion == 1 and
      .component == $component and .version == $version and
      .revision == $revision and .sourceRepository == $repository and
      .fullRevision == $full_revision and
      .versionReference == ($repository + ":" + $version) and
      .revisionReference == ($repository + ":" + $revision) and
      (.sourceDigest | test("^sha256:[0-9a-f]{64}$")) and
      .sourceReference == ($repository + "@" + .sourceDigest) and
      .archive == $archive and .archiveReference == $version and
      (.archiveSha256 | test("^sha256:[0-9a-f]{64}$")) and
      .platforms == $platforms
    ' "$metadata" >/dev/null || die "metadata contract mismatch: $metadata"

  expected_archive_sha=$(jq -r '.archiveSha256' "$metadata")
  [ "$(sha256_file "$archive")" = "$expected_archive_sha" ] \
    || die "archive checksum mismatch: $archive"
  expected_digest=$(jq -r '.sourceDigest' "$metadata")
  raw="$tmp_dir/${component}-archive.raw"
  skopeo_inspect_raw "oci-archive:${archive}:${version}" > "$raw"
  [ -s "$raw" ] || die "archive returned an empty raw root manifest: $archive"
  [ "$(sha256_file "$raw")" = "$expected_digest" ] \
    || die "archive root digest does not match metadata: $archive"
  verify_root_shape "$component" "$raw"
  archive_reference="oci-archive:${archive}:${version}"
  amd64_platform=$(verify_config amd64 "$archive_reference" \
    "$tmp_dir/${component}-amd64.config" "$component archive")
  if [ "$component" = web ]; then
    arm64_platform=$(verify_config arm64 "$archive_reference" \
      "$tmp_dir/${component}-arm64.config" "$component archive")
    actual_platforms=$(jq -cn \
      --arg amd64 "$amd64_platform" --arg arm64 "$arm64_platform" \
      '[$amd64, $arm64]')
  else
    actual_platforms=$(jq -cn --arg amd64 "$amd64_platform" '[$amd64]')
  fi
  jq -en --argjson actual "$actual_platforms" \
    --argjson expected "$expected_platforms" \
    '$actual == $expected' >/dev/null \
    || die "$component archive configurations do not match metadata platforms"
  printf '%s\n' "$expected_digest" > "$tmp_dir/${component}.digest"
done

# Complete the eight-tag preflight before the first copy. An existing equal tag
# is an idempotent success; any mismatch aborts the entire import unchanged.
for component in api web runtime cmaeg; do
  destination_repository="${destination_prefix}/cma-${component}"
  tags="$tmp_dir/${component}-tags.json"
  tags_error="$tmp_dir/${component}-tags.error"
  if ! skopeo_list_tags "docker://${destination_repository}" \
    > "$tags" 2> "$tags_error"; then
    if grep -Eiq \
      'name unknown|repository name not known to registry|repository not found' \
      "$tags_error"; then
      printf '{"Tags":[]}\n' > "$tags"
    else
      die "cannot determine existing tags for $destination_repository"
    fi
  fi
  jq -e '(.Tags == null) or (.Tags | type == "array")' "$tags" >/dev/null \
    || die "registry returned an invalid tag list for $destination_repository"

  expected_digest=$(<"$tmp_dir/${component}.digest")
  for kind in version revision; do
    if [ "$kind" = version ]; then tag=$version; else tag=$revision; fi
    state="$tmp_dir/${component}-${kind}.state"
    if jq -e --arg tag "$tag" '(.Tags // []) | index($tag) != null' \
      "$tags" >/dev/null; then
      raw="$tmp_dir/${component}-${kind}-destination.raw"
      skopeo_inspect_raw "docker://${destination_repository}:${tag}" > "$raw"
      [ -s "$raw" ] || die "destination tag returned an empty manifest: ${destination_repository}:${tag}"
      actual_digest=$(sha256_file "$raw")
      [ "$actual_digest" = "$expected_digest" ] \
        || die "destination tag has a different digest: ${destination_repository}:${tag}"
      verify_destination_configs "$component" \
        "docker://${destination_repository}:${tag}" \
        "$tmp_dir/${component}-${kind}-destination" \
        "destination ${destination_repository}:${tag}"
      printf 'equal\n' > "$state"
    else
      printf 'absent\n' > "$state"
    fi
  done
done

for component in api web runtime cmaeg; do
  base="cma-${component}-${version}-${revision}"
  archive="${component}/${base}.oci.tar"
  destination_repository="${destination_prefix}/cma-${component}"
  expected_digest=$(<"$tmp_dir/${component}.digest")
  for kind in version revision; do
    if [ "$kind" = version ]; then tag=$version; else tag=$revision; fi
    state=$(<"$tmp_dir/${component}-${kind}.state")
    if [ "$state" = equal ]; then
      printf 'Keeping %s:%s at %s\n' "$destination_repository" "$tag" "$expected_digest"
      continue
    fi

    skopeo_copy_all "oci-archive:${archive}:${version}" \
      "docker://${destination_repository}:${tag}"
    raw="$tmp_dir/${component}-${kind}-copied.raw"
    skopeo_inspect_raw "docker://${destination_repository}:${tag}" > "$raw"
    [ -s "$raw" ] || die "copied destination tag returned an empty manifest: ${destination_repository}:${tag}"
    actual_digest=$(sha256_file "$raw")
    [ "$actual_digest" = "$expected_digest" ] \
      || die "copied destination digest does not match source: ${destination_repository}:${tag}"
    verify_destination_configs "$component" \
      "docker://${destination_repository}:${tag}" \
      "$tmp_dir/${component}-${kind}-copied" \
      "copied ${destination_repository}:${tag}"
    printf 'Imported %s:%s at %s\n' "$destination_repository" "$tag" "$expected_digest"
  done
done
