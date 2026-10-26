#!/bin/sh

set -eu

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: %s COMPONENT VERSION FULL_REVISION SOURCE\n' "$0" >&2
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

skopeo_list_tags() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" list-tags --authfile "$skopeo_authfile" "$1"
  else
    "$skopeo_command" list-tags "$1"
  fi
}

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

skopeo_copy_all() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" copy --all --preserve-digests \
      --authfile "$skopeo_authfile" "$1" "$2" 1>&2
  else
    "$skopeo_command" copy --all --preserve-digests "$1" "$2" 1>&2
  fi
}

write_outputs() {
  reuse=$1
  source_digest=$2
  repaired_tag=$3
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    {
      printf 'reuse=%s\n' "$reuse"
      printf 'source_digest=%s\n' "$source_digest"
      printf 'repaired_tag=%s\n' "$repaired_tag"
      printf 'revision=%s\n' "$revision"
    } >> "$GITHUB_OUTPUT"
  else
    printf 'reuse=%s\n' "$reuse"
    printf 'source_digest=%s\n' "$source_digest"
    printf 'repaired_tag=%s\n' "$repaired_tag"
    printf 'revision=%s\n' "$revision"
  fi
}

inspect_raw_to() {
  reference=$1
  destination=$2
  description=$3
  if ! skopeo_inspect_raw "$reference" > "$destination"; then
    die "could not inspect the $description tag"
  fi
  [ -s "$destination" ] || die "$description tag returned an empty raw manifest"
  jq -e '.schemaVersion == 2' "$destination" >/dev/null \
    || die "$description tag returned a malformed manifest"
}

validate_config() {
  architecture=$1
  source_reference=$2
  destination="$tmp_dir/config-${architecture}.json"
  if ! skopeo_inspect_config "$architecture" "$source_reference" > "$destination"; then
    die "could not inspect the linux/$architecture image configuration"
  fi
  [ -s "$destination" ] || die "linux/$architecture returned an empty image configuration"
  jq -e \
    --arg architecture "$architecture" \
    --arg version "$version" \
    --arg revision "$full_revision" \
    '.architecture == $architecture
      and .os == "linux"
      and .config.Labels["org.opencontainers.image.version"] == $version
      and .config.Labels["org.opencontainers.image.revision"] == $revision' \
    "$destination" >/dev/null \
    || die "linux/$architecture does not carry the exact release labels and platform"
}

validate_image() {
  raw_manifest=$1
  digest=$2
  source_reference="docker://${source_repository}@${digest}"

  case "$component" in
    web)
      jq -e '
        (.mediaType == "application/vnd.oci.image.index.v1+json"
          or .mediaType == "application/vnd.docker.distribution.manifest.list.v2+json")
        and (.manifests | type == "array" and length == 2)
        and ([.manifests[] | "\(.platform.os)/\(.platform.architecture)"] | sort
          == ["linux/amd64", "linux/arm64"])' \
        "$raw_manifest" >/dev/null \
        || die "web must be an exact linux/amd64 and linux/arm64 root index"
      validate_config amd64 "$source_reference"
      validate_config arm64 "$source_reference"
      ;;
    api | runtime | cmaeg | image-manager)
      jq -e '
        (.mediaType == "application/vnd.oci.image.manifest.v1+json"
          or .mediaType == "application/vnd.docker.distribution.manifest.v2+json")
        and (has("manifests") | not)' \
        "$raw_manifest" >/dev/null \
        || die "$component must be a plain linux/amd64 image manifest"
      validate_config amd64 "$source_reference"
      ;;
  esac
}

[ "$#" -eq 4 ] || usage

component=$1
version=$2
full_revision=$3
source_repository=$4
skopeo_command=${SKOPEO_COMMAND:-skopeo}
skopeo_authfile=${SKOPEO_AUTHFILE:-}

case "$component" in
  api | web | runtime | cmaeg | image-manager) ;;
  *) die "component must be api, web, runtime, cmaeg, or image-manager" ;;
esac
is_match "$version" '^v[0-9]+\.[0-9]+\.[0-9]+$' \
  || die "version must have the form vMAJOR.MINOR.PATCH"
is_match "$full_revision" '^[0-9a-f]{40}$' \
  || die "revision must be a full 40-character lowercase commit SHA"
is_match "$source_repository" "^ghcr\\.io/[a-z0-9]+([._-][a-z0-9]+)*/cma-${component}$" \
  || die "source must be the matching lowercase GHCR component repository"
command -v "$skopeo_command" >/dev/null 2>&1 \
  || die "Skopeo command is not executable: $skopeo_command"
command -v jq >/dev/null 2>&1 || die "jq is required"

revision=$(printf '%.7s' "$full_revision")
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-reconcile.XXXXXX") \
  || die "cannot create temporary directory"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup 0 1 2 15

tags_file="$tmp_dir/tags.json"
if ! skopeo_list_tags "docker://${source_repository}" > "$tags_file"; then
  die "could not list the source repository tags"
fi
[ -s "$tags_file" ] || die "source repository returned an empty tag listing"
jq -e '.Tags | type == "array" and all(.[]; type == "string")' "$tags_file" >/dev/null \
  || die "source repository returned a malformed tag listing"

version_count=$(jq -r --arg tag "$version" '[.Tags[] | select(. == $tag)] | length' "$tags_file")
revision_count=$(jq -r --arg tag "$revision" '[.Tags[] | select(. == $tag)] | length' "$tags_file")
case "$version_count:$revision_count" in
  0:0)
    write_outputs false '' none
    exit 0
    ;;
  1:1 | 1:0 | 0:1) ;;
  *) die "source repository returned duplicate release tags" ;;
esac

version_raw="$tmp_dir/version.raw"
revision_raw="$tmp_dir/revision.raw"

if [ "$version_count" -eq 1 ]; then
  inspect_raw_to "docker://${source_repository}:${version}" "$version_raw" version
  version_digest=$(sha256_file "$version_raw")
fi
if [ "$revision_count" -eq 1 ]; then
  inspect_raw_to "docker://${source_repository}:${revision}" "$revision_raw" revision
  revision_digest=$(sha256_file "$revision_raw")
fi

if [ "$version_count" -eq 1 ] && [ "$revision_count" -eq 1 ]; then
  [ "$version_digest" = "$revision_digest" ] \
    || die "version and revision tags resolve to different raw manifest digests"
  validate_image "$version_raw" "$version_digest"
  write_outputs true "$version_digest" none
  exit 0
fi

if [ "$version_count" -eq 1 ]; then
  existing_raw=$version_raw
  source_digest=$version_digest
  missing_tag=$revision
  repaired_tag=revision
else
  existing_raw=$revision_raw
  source_digest=$revision_digest
  missing_tag=$version
  repaired_tag=version
fi

validate_image "$existing_raw" "$source_digest"
if ! skopeo_copy_all \
  "docker://${source_repository}@${source_digest}" \
  "docker://${source_repository}:${missing_tag}"; then
  die "could not repair the missing $repaired_tag tag"
fi

repaired_raw="$tmp_dir/repaired.raw"
inspect_raw_to "docker://${source_repository}:${missing_tag}" "$repaired_raw" repaired
repaired_digest=$(sha256_file "$repaired_raw")
[ "$repaired_digest" = "$source_digest" ] \
  || die "the repaired tag does not preserve the source manifest digest"

write_outputs true "$source_digest" "$repaired_tag"
