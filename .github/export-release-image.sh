#!/bin/sh

set -eu

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: %s COMPONENT VERSION FULL_REVISION SOURCE OUTPUT_DIR\n' "$0" >&2
  exit 2
}

is_match() {
  value=$1
  [ "$(printf '%s' "$value" | tr -d '\r\n')" = "$value" ] || return 1
  printf '%s' "$value" | LC_ALL=C grep -Eq "$2"
}

sha256_file() (
  path=$1
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$path") || exit 1
  elif command -v shasum >/dev/null 2>&1; then
    sum=$(shasum -a 256 "$path") || exit 1
  else
    printf 'error: sha256sum or shasum is required\n' >&2
    exit 1
  fi
  sum=${sum%% *}
  is_match "$sum" '^[0-9a-f]{64}$' \
    || die "SHA-256 tool returned an invalid digest for $path"
  printf 'sha256:%s\n' "$sum"
)

skopeo_inspect_raw() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" inspect --raw --authfile "$skopeo_authfile" "$1"
  else
    "$skopeo_command" inspect --raw "$1"
  fi
}

skopeo_inspect_config() {
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

validate_config() {
  architecture=$1
  reference=$2
  destination=$3
  description=$4
  if ! skopeo_inspect_config "$architecture" "$reference" > "$destination"; then
    die "could not inspect the $description linux/$architecture configuration"
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

skopeo_copy_all() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" copy --all --preserve-digests \
      --authfile "$skopeo_authfile" "$1" "$2"
  else
    "$skopeo_command" copy --all --preserve-digests "$1" "$2"
  fi
}

[ "$#" -eq 5 ] || usage

component=$1
version=$2
full_revision=$3
source_repository=$4
output_dir=$5
skopeo_command=${SKOPEO_COMMAND:-skopeo}
skopeo_authfile=${SKOPEO_AUTHFILE:-}

case "$component" in
  api | web | runtime | cmaeg) ;;
  *) die "component must be api, web, runtime, or cmaeg" ;;
esac

is_match "$version" '^v[0-9]+\.[0-9]+\.[0-9]+$' \
  || die "version must have the form vMAJOR.MINOR.PATCH"
is_match "$full_revision" '^[0-9a-f]{40}$' \
  || die "revision must be a full 40-character lowercase commit SHA"
is_match "$source_repository" "^ghcr\\.io/[a-z0-9]+([._-][a-z0-9]+)*/cma-${component}$" \
  || die "source must be the matching lowercase GHCR component repository"

[ -d "$output_dir" ] || die "output directory does not exist: $output_dir"
[ ! -L "$output_dir" ] || die "output directory must not be a symbolic link"
command -v "$skopeo_command" >/dev/null 2>&1 \
  || die "Skopeo command is not executable: $skopeo_command"
command -v jq >/dev/null 2>&1 || die "jq is required"

revision=$(printf '%.7s' "$full_revision")

archive="cma-${component}-${version}-${revision}.oci.tar"
metadata="cma-${component}-${version}-${revision}.json"
archive_reference=$version
version_reference="${source_repository}:${version}"
revision_reference="${source_repository}:${revision}"

output_dir=$(CDPATH='' cd -- "$output_dir" && pwd -P) \
  || die "cannot resolve output directory: $output_dir"
cd "$output_dir"

[ ! -e "$archive" ] && [ ! -L "$archive" ] \
  || die "refusing to overwrite existing archive: $archive"
[ ! -e "$metadata" ] && [ ! -L "$metadata" ] \
  || die "refusing to overwrite existing metadata: $metadata"

stage_dir=$(mktemp -d ".cma-image-export.XXXXXX") \
  || die "cannot create private staging directory"
stage_archive="$stage_dir/$archive"
stage_metadata="$stage_dir/$metadata"

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-export.XXXXXX") \
  || die "cannot create temporary directory"
version_raw="$tmp_dir/version.raw"
revision_raw="$tmp_dir/revision.raw"
archive_raw="$tmp_dir/archive.raw"
complete=0
archive_linked=0
metadata_linked=0

cleanup() {
  rm -rf "$tmp_dir"
  rm -rf "$stage_dir"
  if [ "$complete" -ne 1 ]; then
    [ "$metadata_linked" -ne 1 ] || rm -f "$metadata"
    [ "$archive_linked" -ne 1 ] || rm -f "$archive"
  fi
}
trap cleanup 0 1 2 15

skopeo_inspect_raw "docker://${version_reference}" > "$version_raw"
[ -s "$version_raw" ] || die "version tag returned an empty raw manifest"
skopeo_inspect_raw "docker://${revision_reference}" > "$revision_raw"
[ -s "$revision_raw" ] || die "revision tag returned an empty raw manifest"

version_digest=$(sha256_file "$version_raw")
revision_digest=$(sha256_file "$revision_raw")
if [ "$version_digest" != "$revision_digest" ]; then
  die "version and revision tags resolve to different raw manifest digests"
fi

source_digest=$version_digest
source_reference="${source_repository}@${source_digest}"
if [ "$component" = web ]; then
  jq -e '
    .mediaType == "application/vnd.oci.image.index.v1+json" and
    (.manifests | type == "array" and length == 2) and
    all(.manifests[];
      .mediaType == "application/vnd.oci.image.manifest.v1+json") and
    ([.manifests[].platform | (.os + "/" + .architecture)] | sort ==
     ["linux/amd64", "linux/arm64"])
  ' "$version_raw" >/dev/null \
    || die "web source is not the exact OCI linux/amd64 plus linux/arm64 image index"
  amd64_platform=$(validate_config amd64 "docker://${source_reference}" \
    "$tmp_dir/source-amd64.config" source)
  arm64_platform=$(validate_config arm64 "docker://${source_reference}" \
    "$tmp_dir/source-arm64.config" source)
  platforms=$(jq -cn \
    --arg amd64 "$amd64_platform" --arg arm64 "$arm64_platform" \
    '[$amd64, $arm64]')
else
  jq -e '
    .mediaType == "application/vnd.oci.image.manifest.v1+json" and
    (has("manifests") | not)
  ' "$version_raw" >/dev/null \
    || die "$component source root is not a single OCI image manifest"
  amd64_platform=$(validate_config amd64 "docker://${source_reference}" \
    "$tmp_dir/source-amd64.config" source)
  platforms=$(jq -cn --arg amd64 "$amd64_platform" '[$amd64]')
fi

skopeo_copy_all "docker://${source_reference}" \
  "oci-archive:${stage_archive}:${archive_reference}"
[ -s "$stage_archive" ] || die "Skopeo did not create a non-empty OCI archive"

skopeo_inspect_raw "oci-archive:${stage_archive}:${archive_reference}" > "$archive_raw"
[ -s "$archive_raw" ] || die "OCI archive returned an empty raw root manifest"
archive_root_digest=$(sha256_file "$archive_raw")
if [ "$archive_root_digest" != "$source_digest" ]; then
  die "OCI archive root digest does not match the GHCR source digest"
fi

archive_reference_uri="oci-archive:${stage_archive}:${archive_reference}"
archive_amd64_platform=$(validate_config amd64 "$archive_reference_uri" \
  "$tmp_dir/archive-amd64.config" archive)
[ "$archive_amd64_platform" = "$amd64_platform" ] \
  || die "archive linux/amd64 platform differs from the source"
if [ "$component" = web ]; then
  archive_arm64_platform=$(validate_config arm64 "$archive_reference_uri" \
    "$tmp_dir/archive-arm64.config" archive)
  [ "$archive_arm64_platform" = "$arm64_platform" ] \
    || die "archive linux/arm64 platform differs from the source"
fi

archive_sha256=$(sha256_file "$stage_archive")
cat > "$stage_metadata" <<EOF
{
  "schemaVersion": 1,
  "component": "$component",
  "version": "$version",
  "revision": "$revision",
  "fullRevision": "$full_revision",
  "sourceRepository": "$source_repository",
  "versionReference": "$version_reference",
  "revisionReference": "$revision_reference",
  "sourceDigest": "$source_digest",
  "sourceReference": "$source_reference",
  "archive": "$archive",
  "archiveReference": "$archive_reference",
  "archiveSha256": "$archive_sha256",
  "platforms": $platforms
}
EOF
ln "$stage_archive" "$archive" \
  || die "archive output appeared while the export was running: $archive"
archive_linked=1
ln "$stage_metadata" "$metadata" \
  || die "metadata output appeared while the export was running: $metadata"
metadata_linked=1
complete=1

printf 'Exported %s as %s with root digest %s\n' \
  "$source_reference" "$archive" "$source_digest"
