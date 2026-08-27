#!/usr/bin/env bash

set -euo pipefail

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: %s tag|release VERSION FULL_REVISION NOTES_FILE\n' "$0" >&2
  exit 2
}

is_match() {
  local value
  value=$1
  [ "$(printf '%s' "$value" | tr -d '\r\n')" = "$value" ] || return 1
  printf '%s' "$value" | LC_ALL=C grep -Eq "$2"
}

extract_tag_message() {
  reference=$1
  destination=$2
  if ! "$git_command" cat-file tag "$reference" | {
    header_ended=0
    while IFS= read -r line; do
      if [ -z "$line" ]; then
        header_ended=1
        break
      fi
    done
    [ "$header_ended" -eq 1 ] || exit 1
    cat
  } > "$destination"; then
    die "the existing annotated tag object is malformed"
  fi
}

verify_tag() {
  reference=$1
  object_type=$("$git_command" cat-file -t "$reference") \
    || die "the existing tag object cannot be read"
  [ "$object_type" = tag ] || die "$version exists but is not an annotated tag"

  embedded_name=$("$git_command" for-each-ref --format='%(tag)' "$reference") \
    || die "the existing annotated tag name cannot be read"
  [ "$embedded_name" = "$version" ] \
    || die "the existing annotated tag object names $embedded_name instead of $version"

  dereferenced=$("$git_command" rev-parse "${reference}^{}") \
    || die "the existing annotated tag target cannot be resolved"
  is_match "$dereferenced" '^[0-9a-f]{40}$' \
    || die "the existing annotated tag target is not a commit SHA"
  target_type=$("$git_command" cat-file -t "$dereferenced") \
    || die "the existing annotated tag target cannot be read"
  [ "$target_type" = commit ] || die "the existing annotated tag does not target a commit"
  [ "$dereferenced" = "$full_revision" ] \
    || die "the existing annotated tag targets $dereferenced instead of $full_revision"

  extract_tag_message "$reference" "$tmp_dir/tag-message"
  cmp -s "$notes_file" "$tmp_dir/tag-message" \
    || die "the existing annotated tag message differs from the assembled release notes"
}

reconcile_tag() {
  remote_ref="refs/tags/$version"
  set +e
  "$git_command" ls-remote --exit-code --tags origin "$remote_ref" \
    > "$tmp_dir/remote-tag" 2> "$tmp_dir/remote-tag-error"
  remote_status=$?
  set -e

  case "$remote_status" in
    0)
      [ "$(wc -l < "$tmp_dir/remote-tag" | tr -d ' ')" -eq 1 ] \
        || die "the remote returned an ambiguous $version tag"
      remote_object=$(awk '{print $1}' "$tmp_dir/remote-tag")
      listed_ref=$(awk '{print $2}' "$tmp_dir/remote-tag")
      is_match "$remote_object" '^[0-9a-f]{40}$' \
        || die "the remote returned an invalid $version tag object"
      [ "$listed_ref" = "$remote_ref" ] \
        || die "the remote returned an unexpected tag reference"
      "$git_command" fetch --force --no-tags origin "$remote_ref:$remote_ref" \
        >/dev/null 2>&1 \
        || die "the existing remote tag could not be fetched for verification"
      fetched_object=$("$git_command" rev-parse "$remote_ref") \
        || die "the fetched remote tag cannot be resolved"
      [ "$fetched_object" = "$remote_object" ] \
        || die "the remote tag changed while it was being verified"
      verify_tag "$remote_ref"
      printf 'Reusing exact annotated tag %s\n' "$version"
      ;;
    2)
      set +e
      "$git_command" show-ref --verify --quiet "$remote_ref"
      local_status=$?
      set -e
      case "$local_status" in
        0) die "$version exists locally but not on the remote" ;;
        1) ;;
        *) die "the local tag namespace could not be checked" ;;
      esac
      "$git_command" tag -a --cleanup=verbatim \
        "$version" "$full_revision" -F "$notes_file"
      verify_tag "$remote_ref"
      "$git_command" push origin "$remote_ref"
      printf 'Created annotated tag %s\n' "$version"
      ;;
    *)
      cat "$tmp_dir/remote-tag-error" >&2
      die "the remote tag namespace could not be read"
      ;;
  esac
}

reconcile_release() {
  : "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
  is_match "$GITHUB_REPOSITORY" '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' \
    || die "GITHUB_REPOSITORY is invalid"
  command -v jq >/dev/null 2>&1 || die "jq is required"

  if ! "$gh_command" api --paginate --slurp \
    "repos/${GITHUB_REPOSITORY}/releases?per_page=100" \
    > "$tmp_dir/releases.json"; then
    die "the repository releases could not be listed"
  fi
  jq -e 'type == "array" and all(.[]; type == "array")' \
    "$tmp_dir/releases.json" >/dev/null \
    || die "GitHub returned a malformed release listing"

  matches=$(jq -r --arg version "$version" \
    '[.[][] | select(.tag_name == $version)] | length' \
    "$tmp_dir/releases.json")
  case "$matches" in
    0)
      "$gh_command" release create "$version" \
        --title "$version" \
        --notes-file "$notes_file" \
        --verify-tag
      printf 'Created GitHub release %s\n' "$version"
      ;;
    1)
      jq -e --arg version "$version" '
        [.[][] | select(.tag_name == $version)][0] as $release
        | $release.tag_name == $version
          and $release.name == $version
          and ($release.body | type == "string")
          and $release.draft == false
          and $release.prerelease == false' \
        "$tmp_dir/releases.json" >/dev/null \
        || die "the existing GitHub release metadata differs from the requested release"
      jq -j --arg version "$version" \
        '[.[][] | select(.tag_name == $version)][0].body' \
        "$tmp_dir/releases.json" > "$tmp_dir/release-body"
      cmp -s "$notes_file" "$tmp_dir/release-body" \
        || die "the existing GitHub release body differs from the assembled release notes"
      printf 'Reusing exact GitHub release %s\n' "$version"
      ;;
    *) die "GitHub returned more than one release for $version" ;;
  esac
}

[ "$#" -eq 4 ] || usage

mode=$1
version=$2
full_revision=$3
notes_file=$4
git_command=${GIT_COMMAND:-git}
gh_command=${GH_COMMAND:-gh}

case "$mode" in
  tag | release) ;;
  *) usage ;;
esac
is_match "$version" '^v[0-9]+\.[0-9]+\.[0-9]+$' \
  || die "version must have the form vMAJOR.MINOR.PATCH"
is_match "$full_revision" '^[0-9a-f]{40}$' \
  || die "revision must be a full 40-character lowercase commit SHA"
[ -f "$notes_file" ] && [ ! -L "$notes_file" ] && [ -s "$notes_file" ] \
  || die "release notes must be a non-empty regular file"

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-release-reconcile.XXXXXX") \
  || die "cannot create temporary directory"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT HUP INT TERM

case "$mode" in
  tag)
    command -v "$git_command" >/dev/null 2>&1 \
      || die "Git command is not executable: $git_command"
    reconcile_tag
    ;;
  release)
    command -v "$gh_command" >/dev/null 2>&1 \
      || die "GitHub command is not executable: $gh_command"
    reconcile_release
    ;;
esac
