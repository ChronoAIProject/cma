#!/usr/bin/env bash

set -euo pipefail

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf 'usage: %s LEVEL FULL_REVISION OWNER SERVER_URL NOTES_FILE OUTPUT_FILE\n' "$0" >&2
  exit 2
}

is_match() {
  local value
  value=$1
  [ "$(printf '%s' "$value" | tr -d '\r\n')" = "$value" ] || return 1
  printf '%s' "$value" | LC_ALL=C grep -Eq "$2"
}

parse_semver() {
  local value parts
  value=$1
  parts=${value#v}
  IFS=. read -r semver_major semver_minor semver_patch <<<"$parts"
}

compare_decimal() {
  local left right
  left=$1
  right=$2
  if ((${#left} < ${#right})); then
    decimal_comparison=-1
  elif ((${#left} > ${#right})); then
    decimal_comparison=1
  elif [[ "$left" < "$right" ]]; then
    decimal_comparison=-1
  elif [[ "$left" > "$right" ]]; then
    decimal_comparison=1
  else
    decimal_comparison=0
  fi
}

compare_semver() {
  local left right left_major left_minor left_patch
  local right_major right_minor right_patch
  left=$1
  right=$2

  parse_semver "$left"
  left_major=$semver_major
  left_minor=$semver_minor
  left_patch=$semver_patch
  parse_semver "$right"
  right_major=$semver_major
  right_minor=$semver_minor
  right_patch=$semver_patch

  compare_decimal "$left_major" "$right_major"
  semver_comparison=$decimal_comparison
  [ "$semver_comparison" -eq 0 ] || return 0
  compare_decimal "$left_minor" "$right_minor"
  semver_comparison=$decimal_comparison
  [ "$semver_comparison" -eq 0 ] || return 0
  compare_decimal "$left_patch" "$right_patch"
  semver_comparison=$decimal_comparison
}

increment_decimal() {
  local value result digit next index carry
  value=$1
  result=
  carry=1
  for ((index = ${#value} - 1; index >= 0; index -= 1)); do
    digit=${value:index:1}
    if [ "$carry" -eq 1 ]; then
      case "$digit" in
        0) next=1; carry=0 ;;
        1) next=2; carry=0 ;;
        2) next=3; carry=0 ;;
        3) next=4; carry=0 ;;
        4) next=5; carry=0 ;;
        5) next=6; carry=0 ;;
        6) next=7; carry=0 ;;
        7) next=8; carry=0 ;;
        8) next=9; carry=0 ;;
        9) next=0 ;;
        *) die "invalid decimal component: $value" ;;
      esac
    else
      next=$digit
    fi
    result="${next}${result}"
  done
  [ "$carry" -eq 0 ] || result="1${result}"
  printf '%s\n' "$result"
}

bump_version() {
  local base level major minor patch
  base=$1
  level=$2
  parse_semver "$base"
  major=$semver_major
  minor=$semver_minor
  patch=$semver_patch
  case "$level" in
    major)
      major=$(increment_decimal "$major")
      minor=0
      patch=0
      ;;
    minor)
      minor=$(increment_decimal "$minor")
      patch=0
      ;;
    patch) patch=$(increment_decimal "$patch") ;;
  esac
  printf 'v%s.%s.%s\n' "$major" "$minor" "$patch"
}

write_output() {
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'version=%s\n' "$version" >> "$GITHUB_OUTPUT"
  else
    printf 'version=%s\n' "$version"
  fi
}

[ "$#" -eq 6 ] || usage

level=$1
full_revision=$2
owner=$3
server_url=${4%/}
notes_file=$5
output_file=$6
git_command=${GIT_COMMAND:-git}
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
reconcile_script="$script_dir/reconcile-github-release.sh"

case "$level" in
  major | minor | patch) ;;
  *) die "level must be major, minor, or patch" ;;
esac
is_match "$full_revision" '^[0-9a-f]{40}$' \
  || die "revision must be a full 40-character lowercase commit SHA"
is_match "$owner" '^[A-Za-z0-9_.-]+$' || die "owner is invalid"
is_match "$server_url" '^https://[^[:space:]]+$' || die "server URL is invalid"
[ -f "$notes_file" ] && [ ! -L "$notes_file" ] && [ -s "$notes_file" ] \
  || die "release notes must be a non-empty regular file"
[ ! -e "$output_file" ] && [ ! -L "$output_file" ] \
  || die "refusing to overwrite the assembled release notes"
[ -d "$(dirname -- "$output_file")" ] \
  || die "assembled release notes directory does not exist"
command -v "$git_command" >/dev/null 2>&1 \
  || die "Git command is not executable: $git_command"
[ -x "$reconcile_script" ] || die "release reconciler is not executable"

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/cma-release-plan.XXXXXX") \
  || die "cannot create temporary directory"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT HUP INT TERM

all_tags="$tmp_dir/all-tags"
semver_tags="$tmp_dir/semver-tags"
"$git_command" tag --list 'v*' > "$all_tags" \
  || die "the local tag namespace could not be listed"
set +e
LC_ALL=C grep -E '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
  "$all_tags" > "$semver_tags"
grep_status=$?
set -e
case "$grep_status" in
  0 | 1) ;;
  *) die "canonical release tags could not be selected" ;;
esac

declare -a current_tags=()
latest=v0.0.0
while IFS= read -r tag; do
  [ -n "$tag" ] || continue
  target=$("$git_command" rev-parse --verify "${tag}^{}") \
    || die "$tag cannot be resolved"
  is_match "$target" '^[0-9a-f]{40}$' \
    || die "$tag does not resolve to a commit SHA"
  [ "$("$git_command" cat-file -t "$target")" = commit ] \
    || die "$tag does not resolve to a commit"
  compare_semver "$tag" "$latest"
  if [ "$semver_comparison" -gt 0 ]; then latest=$tag; fi
  if [ "$target" = "$full_revision" ]; then current_tags+=("$tag"); fi
done < "$semver_tags"

case "${#current_tags[@]}" in
  0)
    if [ "$latest" != v0.0.0 ]; then
      latest_revision=$("$git_command" rev-parse --verify "${latest}^{}") \
        || die "$latest cannot be resolved"
      if "$git_command" merge-base --is-ancestor "$latest_revision" "$full_revision"; then
        :
      elif "$git_command" merge-base --is-ancestor "$full_revision" "$latest_revision"; then
        die "$full_revision was superseded by the already-released $latest commit"
      else
        die "$full_revision is not descended from the latest release $latest"
      fi
    fi
    version=$(bump_version "$latest" "$level")
    ;;
  1)
    version=${current_tags[0]}
    [ "$("$git_command" cat-file -t "refs/tags/$version")" = tag ] \
      || die "$version exists on this commit but is not an annotated tag"

    predecessor=v0.0.0
    while IFS= read -r tag; do
      [ -n "$tag" ] || continue
      [ "$tag" != "$version" ] || continue
      compare_semver "$tag" "$version"
      [ "$semver_comparison" -lt 0 ] || continue
      compare_semver "$tag" "$predecessor"
      if [ "$semver_comparison" -gt 0 ]; then predecessor=$tag; fi
    done < "$semver_tags"
    expected=$(bump_version "$predecessor" "$level")
    [ "$version" = "$expected" ] \
      || die "$version is not the declared $level bump from $predecessor (expected $expected)"
    if [ "$predecessor" != v0.0.0 ]; then
      predecessor_revision=$("$git_command" rev-parse --verify "${predecessor}^{}") \
        || die "$predecessor cannot be resolved"
      "$git_command" merge-base --is-ancestor \
        "$predecessor_revision" "$full_revision" \
        || die "$version is not descended from its numeric predecessor $predecessor"
    fi
    ;;
  *) die "more than one canonical release tag resolves to $full_revision" ;;
esac

owner_lower=$(printf '%s' "$owner" | tr '[:upper:]' '[:lower:]')
assembled="$tmp_dir/release.md"
printf '## Release Notes %s\n\n' "$version" > "$assembled"
cat "$notes_file" >> "$assembled"
{
  printf '\n### Images\n\n'
  printf 'Published to GitHub Container Registry, private to the organisation.\n\n'
  printf '```\n'
  for component in api web runtime cmaeg image-manager; do
    printf 'ghcr.io/%s/cma-%s:%s\n' "$owner_lower" "$component" "$version"
  done
  printf '```\n\n'
  for component in api web runtime cmaeg image-manager; do
    printf -- '- [cma-%s](%s/orgs/%s/packages/container/package/cma-%s)\n' \
      "$component" "$server_url" "$owner" "$component"
  done
} >> "$assembled"

if [ "${#current_tags[@]}" -eq 1 ]; then
  GIT_COMMAND="$git_command" \
    "$reconcile_script" tag "$version" "$full_revision" "$assembled" \
    >/dev/null
fi

mv "$assembled" "$output_file"
write_output
printf 'Planned %s (%s bump, commit %s)\n' "$version" "$level" "$full_revision"
