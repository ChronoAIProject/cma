#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
reconcile_script="$script_dir/reconcile-github-release.sh"
suite_tmp=$(mktemp -d "${TMPDIR:-/tmp}/cma-github-release-reconcile-tests.XXXXXX")
real_git=$(command -v git)
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

git_mock="$suite_tmp/git"
cat > "$git_mock" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

command_name=$1
{
  printf '%s' "$command_name"
  shift
  for argument in "$@"; do
    printf '\t%s' "$argument"
  done
  printf '\n'
} >> "$GIT_TEST_LOG"

if [ "${GIT_TEST_FAIL_LS_REMOTE:-false}" = true ] \
  && [ "$command_name" = ls-remote ]; then
  printf 'simulated ls-remote failure\n' >&2
  exit 70
fi

exec "$REAL_GIT" "$command_name" "$@"
MOCK
chmod 0755 "$git_mock"

tag_case_number=0
new_tag_case() {
  tag_case_number=$((tag_case_number + 1))
  case_dir="$suite_tmp/tag-$tag_case_number"
  remote="$case_dir/remote.git"
  repository="$case_dir/repository"
  notes="$case_dir/release.md"
  git_log="$case_dir/git.log"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"
  version=v1.2.3
  fail_ls_remote=false

  mkdir -p "$case_dir"
  "$real_git" init --bare --object-format=sha1 -q "$remote"
  "$real_git" init --object-format=sha1 -q "$repository"
  "$real_git" -C "$repository" config user.name 'Release Test'
  "$real_git" -C "$repository" config user.email release-test@example.invalid
  "$real_git" -C "$repository" config commit.gpgSign false
  "$real_git" -C "$repository" config tag.gpgSign false
  printf 'first\n' > "$repository/content.txt"
  "$real_git" -C "$repository" add content.txt
  "$real_git" -C "$repository" commit -q -m first
  full_revision=$("$real_git" -C "$repository" rev-parse HEAD)
  [ "${#full_revision}" -eq 40 ] || fail "tag fixture did not use SHA-1"
  "$real_git" -C "$repository" remote add origin "$remote"
  "$real_git" -C "$repository" push -q origin HEAD:refs/heads/main
  printf '## Release Notes %s\n\n- exact bytes\n' "$version" > "$notes"
  : > "$git_log"
}

run_tag() {
  (
    cd "$repository"
    env \
      GIT_COMMAND="$git_mock" \
      GIT_TEST_LOG="$git_log" \
      GIT_TEST_FAIL_LS_REMOTE="$fail_ls_remote" \
      REAL_GIT="$real_git" \
      "$reconcile_script" tag "$version" "$full_revision" "$notes"
  ) > "$stdout" 2> "$stderr"
}

git_push_count() {
  awk -F '\t' '$1 == "push" { count += 1 } END { print count + 0 }' "$git_log"
}

assert_no_git_push() {
  [ "$(git_push_count)" -eq 0 ] \
    || fail "a rejected tag state pushed to the remote"
}

seed_annotated_tag() {
  target=$1
  message_file=$2
  "$real_git" -C "$repository" tag -a --cleanup=verbatim \
    "$version" "$target" -F "$message_file"
  "$real_git" -C "$repository" push -q origin "refs/tags/$version"
}

new_tag_case
run_tag || fail "an absent remote tag was not created"
[ "$(git_push_count)" -eq 1 ] || fail "new tag did not perform exactly one push"
[ "$("$real_git" --git-dir="$remote" cat-file -t "refs/tags/$version")" = tag ] \
  || fail "new remote tag is not annotated"
[ "$("$real_git" --git-dir="$remote" rev-parse "refs/tags/$version^{}")" = "$full_revision" ] \
  || fail "new remote tag does not target the requested commit"
"$real_git" --git-dir="$remote" cat-file tag "refs/tags/$version" \
  | sed '1,/^$/d' > "$case_dir/tag-message"
cmp -s "$notes" "$case_dir/tag-message" \
  || fail "new remote tag did not preserve the release notes bytes"
pass "an absent tag is created as an exact annotated tag and pushed once"

new_tag_case
seed_annotated_tag "$full_revision" "$notes"
: > "$git_log"
run_tag || fail "an exact existing remote tag was not reused"
assert_no_git_push
assert_contains "$stdout" "Reusing exact annotated tag $version"
pass "an exact existing annotated tag is reused without another push"

new_tag_case
"$real_git" -C "$repository" tag "$version" "$full_revision"
"$real_git" -C "$repository" push -q origin "refs/tags/$version"
: > "$git_log"
if run_tag; then fail "a lightweight remote tag was accepted"; fi
assert_no_git_push
assert_contains "$stderr" "is not an annotated tag"
pass "a lightweight tag fails closed without a push"

new_tag_case
first_revision=$full_revision
printf 'second\n' >> "$repository/content.txt"
"$real_git" -C "$repository" commit -q -am second
full_revision=$("$real_git" -C "$repository" rev-parse HEAD)
seed_annotated_tag "$first_revision" "$notes"
: > "$git_log"
if run_tag; then fail "an annotated tag targeting another commit was accepted"; fi
assert_no_git_push
assert_contains "$stderr" "instead of $full_revision"
pass "an annotated tag targeting another commit fails closed"

new_tag_case
printf 'different annotation\n' > "$case_dir/different.md"
seed_annotated_tag "$full_revision" "$case_dir/different.md"
: > "$git_log"
if run_tag; then fail "an annotated tag with different bytes was accepted"; fi
assert_no_git_push
assert_contains "$stderr" "tag message differs"
pass "an annotation-byte mismatch fails closed"

new_tag_case
fail_ls_remote=true
if run_tag; then fail "a remote tag lookup failure was treated as absence"; fi
assert_no_git_push
assert_contains "$stderr" "remote tag namespace could not be read"
pass "a remote lookup failure never creates or pushes a tag"

new_tag_case
version=$(printf '../../escape\nv1.2.3')
if run_tag; then fail "newline path traversal in the tag version was accepted"; fi
assert_no_git_push
[ ! -s "$git_log" ] || fail "newline path traversal invoked Git"
pass "newline path traversal cannot bypass release-tag validation"

gh_mock="$suite_tmp/gh"
cat > "$gh_mock" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail

command_name=$1
{
  printf '%s' "$command_name"
  shift
  for argument in "$@"; do
    printf '\t%s' "$argument"
  done
  printf '\n'
} >> "$GH_TEST_LOG"

case "$GH_TEST_MODE:$command_name" in
  api-failure:api)
    printf 'simulated GitHub API failure\n' >&2
    exit 71
    ;;
  *:api)
    cat "$GH_TEST_RESPONSE"
    ;;
  *:release)
    [ "${1:-}" = create ] || exit 72
    ;;
  *) exit 73 ;;
esac
MOCK
chmod 0755 "$gh_mock"

release_case_number=0
new_release_case() {
  release_case_number=$((release_case_number + 1))
  case_dir="$suite_tmp/release-$release_case_number"
  notes="$case_dir/release.md"
  response="$case_dir/releases.json"
  gh_log="$case_dir/gh.log"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"
  version=v1.2.3
  full_revision=0123456789abcdef0123456789abcdef01234567
  gh_mode=normal

  mkdir -p "$case_dir"
  printf '## Release Notes %s\n\n- exact bytes\n' "$version" > "$notes"
  printf '[[]]\n' > "$response"
  : > "$gh_log"
}

run_release() {
  env \
    GH_COMMAND="$gh_mock" \
    GH_TEST_LOG="$gh_log" \
    GH_TEST_MODE="$gh_mode" \
    GH_TEST_RESPONSE="$response" \
    GITHUB_REPOSITORY=ChronoAIProject/cma \
    "$reconcile_script" release "$version" "$full_revision" "$notes" \
    > "$stdout" 2> "$stderr"
}

gh_mutation_count() {
  awk -F '\t' \
    '$1 == "release" && ($2 == "create" || $2 == "edit") { count += 1 }
     END { print count + 0 }' "$gh_log"
}

assert_no_gh_mutation() {
  [ "$(gh_mutation_count)" -eq 0 ] \
    || fail "a rejected GitHub release state was mutated"
}

write_release_response() {
  title=$1
  body_file=$2
  draft=$3
  prerelease=$4
  jq -n \
    --arg tag "$version" \
    --arg title "$title" \
    --rawfile body "$body_file" \
    --argjson draft "$draft" \
    --argjson prerelease "$prerelease" \
    '[[{
      tag_name: $tag,
      name: $title,
      body: $body,
      draft: $draft,
      prerelease: $prerelease
    }]]' > "$response"
}

new_release_case
run_release || fail "an absent GitHub release was not created"
[ "$(gh_mutation_count)" -eq 1 ] \
  || fail "an absent release did not perform exactly one create"
assert_contains "$gh_log" "release	create	$version	--title	$version	--notes-file	$notes	--verify-tag"
pass "an absent GitHub release is created with the exact immutable tag and notes"

new_release_case
write_release_response "$version" "$notes" false false
run_release || fail "an exact existing GitHub release was not reused"
assert_no_gh_mutation
assert_contains "$stdout" "Reusing exact GitHub release $version"
pass "an exact existing GitHub release is reused without create or edit"

new_release_case
write_release_response 'wrong title' "$notes" false false
if run_release; then fail "a release with the wrong title was accepted"; fi
assert_no_gh_mutation
pass "a title mismatch fails closed without create or edit"

new_release_case
printf 'different body\n' > "$case_dir/different.md"
write_release_response "$version" "$case_dir/different.md" false false
if run_release; then fail "a release with a different body was accepted"; fi
assert_no_gh_mutation
assert_contains "$stderr" "release body differs"
pass "a release-body byte mismatch fails closed without create or edit"

new_release_case
write_release_response "$version" "$notes" true false
if run_release; then fail "a draft release was accepted"; fi
assert_no_gh_mutation
pass "a draft-state mismatch fails closed without create or edit"

new_release_case
write_release_response "$version" "$notes" false true
if run_release; then fail "a prerelease was accepted"; fi
assert_no_gh_mutation
pass "a prerelease-state mismatch fails closed without create or edit"

new_release_case
gh_mode=api-failure
if run_release; then fail "a GitHub API failure was treated as release absence"; fi
assert_no_gh_mutation
assert_contains "$stderr" "releases could not be listed"
pass "a GitHub API failure never creates or edits a release"

printf '1..%d\n' "$tests"
