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

skopeo_list_tags() {
  if [ -n "$skopeo_authfile" ]; then
    "$skopeo_command" list-tags --authfile "$skopeo_authfile" "$1"
  else
    "$skopeo_command" list-tags "$1"
  fi
}

[ "$#" -eq 4 ] || usage

component=$1
version=$2
full_revision=$3
source_repository=$4
skopeo_command=${SKOPEO_COMMAND:-skopeo}
skopeo_authfile=${SKOPEO_AUTHFILE:-}

case "$component" in
  api | web | runtime) ;;
  *) die "component must be api, web, or runtime" ;;
esac
is_match "$version" '^v[0-9]+\.[0-9]+\.[0-9]+$' \
  || die "version must have the form vMAJOR.MINOR.PATCH"
is_match "$full_revision" '^[0-9a-f]{40}$' \
  || die "revision must be a full 40-character lowercase commit SHA"
is_match "$source_repository" \
  "^ghcr\\.io/[a-z0-9]+([._-][a-z0-9]+)*/cma-${component}$" \
  || die "source must be the matching lowercase GHCR component repository"
command -v "$skopeo_command" >/dev/null 2>&1 \
  || die "Skopeo command is not executable: $skopeo_command"
command -v jq >/dev/null 2>&1 || die "jq is required"

revision=$(printf '%.7s' "$full_revision")
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-image-absence.XXXXXX") \
  || die "cannot create temporary directory"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup 0 1 2 15

tags_file="$tmp_dir/tags.json"
if ! skopeo_list_tags "docker://${source_repository}" > "$tags_file"; then
  die "could not list the source repository tags immediately before publish"
fi
[ -s "$tags_file" ] \
  || die "source repository returned an empty tag listing immediately before publish"
jq -e '.Tags | type == "array" and all(.[]; type == "string")' \
  "$tags_file" >/dev/null \
  || die "source repository returned a malformed tag listing immediately before publish"

present=$(jq -r --arg version "$version" --arg revision "$revision" '
  [.Tags[] | select(. == $version or . == $revision)] | unique | join(", ")
' "$tags_file")
[ -z "$present" ] \
  || die "release image tag appeared after reconciliation: $present"

printf 'Confirmed %s:%s and %s:%s are still absent\n' \
  "$source_repository" "$version" "$source_repository" "$revision"
