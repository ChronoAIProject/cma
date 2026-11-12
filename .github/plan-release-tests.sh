#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
plan_script="$script_dir/plan-release.sh"
release_script="$script_dir/reconcile-github-release.sh"
real_git=$(command -v git)
suite_tmp=$(mktemp -d "${TMPDIR:-/tmp}/cma-release-plan-tests.XXXXXX")
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
  local file text
  file=$1
  text=$2
  grep -F -- "$text" "$file" >/dev/null \
    || fail "$file does not contain: $text"
}

case_number=0
new_case() {
  case_number=$((case_number + 1))
  case_dir="$suite_tmp/case-$case_number"
  remote="$case_dir/remote.git"
  repository="$case_dir/repository"
  notes="$case_dir/notes.md"
  output="$case_dir/release.md"
  github_output="$case_dir/github-output"
  stdout="$case_dir/stdout"
  stderr="$case_dir/stderr"

  mkdir -p "$case_dir"
  "$real_git" init --bare --object-format=sha1 -q "$remote"
  "$real_git" init --object-format=sha1 -q "$repository"
  "$real_git" -C "$repository" config user.name 'Release Plan Test'
  "$real_git" -C "$repository" config user.email release-plan@example.invalid
  "$real_git" -C "$repository" config commit.gpgSign false
  "$real_git" -C "$repository" config tag.gpgSign false
  "$real_git" -C "$repository" remote add origin "$remote"

  printf 'base\n' > "$repository/content.txt"
  "$real_git" -C "$repository" add content.txt
  "$real_git" -C "$repository" commit -q -m base
  base_revision=$("$real_git" -C "$repository" rev-parse HEAD)
  printf 'release\n' >> "$repository/content.txt"
  "$real_git" -C "$repository" commit -q -am release
  full_revision=$("$real_git" -C "$repository" rev-parse HEAD)
  "$real_git" -C "$repository" push -q origin HEAD:refs/heads/main

  printf '%s\n' \
    "### What's New" '- exact feature' '' \
    "### What's Fixed" '- exact fix' '' \
    "### What's Changed" '- exact change' > "$notes"
  : > "$github_output"
}

seed_annotated_tag() {
  local version target message
  version=$1
  target=$2
  message=$3
  "$real_git" -C "$repository" tag -a --cleanup=verbatim \
    "$version" "$target" -F "$message"
  "$real_git" -C "$repository" push -q origin "refs/tags/$version"
}

run_plan() {
  local level
  level=$1
  (
    cd "$repository"
    env GITHUB_OUTPUT="$github_output" GIT_COMMAND="$real_git" \
      "$plan_script" "$level" "$full_revision" ChronoAIProject \
        https://github.com "$notes" "$output"
  ) > "$stdout" 2> "$stderr"
}

planned_version() {
  sed -n 's/^version=//p' "$github_output"
}

new_case
run_plan patch || fail "the first canonical patch release was not planned"
[ "$(planned_version)" = v0.0.1 ] || fail "the seed patch version is wrong"
assert_contains "$output" '## Release Notes v0.0.1'
assert_contains "$output" 'ghcr.io/chronoaiproject/cma-web:v0.0.1'
assert_contains "$output" 'ghcr.io/chronoaiproject/cma-cmaeg:v0.0.1'
assert_contains "$output" 'ghcr.io/chronoaiproject/cma-api:v0.0.1'
assert_contains "$output" 'ghcr.io/chronoaiproject/cma-runtime:v0.0.1'
[ "$(grep -c '^ghcr.io/chronoaiproject/cma-' "$output")" -eq 4 ] \
  || fail "release notes must list exactly four service images"
pass "an untagged first release is assembled as v0.0.1"

new_case
printf 'older\n' > "$case_dir/older.md"
seed_annotated_tag v2.9.9 "$base_revision" "$case_dir/older.md"
seed_annotated_tag v2.10.0 "$base_revision" "$case_dir/older.md"
run_plan patch || fail "numeric semver ordering did not plan a release"
[ "$(planned_version)" = v2.10.1 ] \
  || fail "numeric semver ordering did not choose v2.10.0"
pass "the fresh bump uses numeric rather than lexical semver ordering"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v1.2.3 "$base_revision" "$case_dir/predecessor.md"
run_plan minor || fail "the initial minor release was not planned"
expected="$case_dir/expected.md"
mv "$output" "$expected"
seed_annotated_tag v1.3.0 "$full_revision" "$expected"
: > "$github_output"
run_plan minor || fail "a tag-only rerun did not reuse the exact version"
[ "$(planned_version)" = v1.3.0 ] || fail "tag-only rerun changed the version"
cmp -s "$expected" "$output" || fail "tag-only rerun changed the release bytes"
pass "rerun-all after tag creation reuses its exact version and annotation"

gh_mock="$suite_tmp/gh"
cat > "$gh_mock" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
command_name=$1
shift
{
  printf '%s' "$command_name"
  for argument in "$@"; do printf '\t%s' "$argument"; done
  printf '\n'
} >> "$GH_TEST_LOG"
case "$command_name" in
  api) cat "$GH_TEST_RESPONSE" ;;
  release) [ "${1:-}" = create ] ;;
  *) exit 72 ;;
esac
MOCK
chmod 0755 "$gh_mock"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v3.4.5 "$base_revision" "$case_dir/predecessor.md"
run_plan patch || fail "the complete-release fixture was not planned"
expected="$case_dir/expected.md"
mv "$output" "$expected"
seed_annotated_tag v3.4.6 "$full_revision" "$expected"
: > "$github_output"
run_plan patch || fail "the complete release rerun did not reuse its version"
response="$case_dir/releases.json"
gh_log="$case_dir/gh.log"
jq -n --arg tag v3.4.6 --rawfile body "$output" \
  '[[{tag_name:$tag,name:$tag,body:$body,draft:false,prerelease:false}]]' \
  > "$response"
: > "$gh_log"
env GH_COMMAND="$gh_mock" GH_TEST_LOG="$gh_log" \
  GH_TEST_RESPONSE="$response" GITHUB_REPOSITORY=ChronoAIProject/cma \
  "$release_script" release v3.4.6 "$full_revision" "$output" \
  >/dev/null 2> "$stderr" \
  || fail "the exact completed release was not reused"
if awk -F '\t' '$1 == "release" { found=1 } END { exit found ? 0 : 1 }' "$gh_log"; then
  fail "the exact completed release was mutated"
fi
pass "rerun-all after a complete tag and release is an exact no-op"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
printf 'wrong annotation\n' > "$case_dir/wrong.md"
seed_annotated_tag v4.0.0 "$base_revision" "$case_dir/predecessor.md"
seed_annotated_tag v4.1.0 "$full_revision" "$case_dir/wrong.md"
if run_plan minor; then fail "a current tag with the wrong annotation was accepted"; fi
assert_contains "$stderr" 'tag message differs'
pass "a current-commit annotation conflict fails closed"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v5.0.0 "$base_revision" "$case_dir/predecessor.md"
seed_annotated_tag v5.2.0 "$full_revision" "$case_dir/predecessor.md"
if run_plan minor; then fail "a current tag with the wrong bump was accepted"; fi
assert_contains "$stderr" 'is not the declared minor bump'
pass "a current-commit bump conflict fails closed"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v6.0.0 "$base_revision" "$case_dir/predecessor.md"
"$real_git" -C "$repository" tag v6.0.1 "$full_revision"
"$real_git" -C "$repository" push -q origin refs/tags/v6.0.1
if run_plan patch; then fail "a lightweight current release tag was accepted"; fi
assert_contains "$stderr" 'is not an annotated tag'
pass "a lightweight current-commit tag fails closed"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v7.0.0 "$base_revision" "$case_dir/predecessor.md"
seed_annotated_tag v7.0.1 "$full_revision" "$case_dir/predecessor.md"
seed_annotated_tag v7.0.2 "$full_revision" "$case_dir/predecessor.md"
if run_plan patch; then fail "multiple current release tags were accepted"; fi
assert_contains "$stderr" 'more than one canonical release tag'
pass "multiple canonical tags on the current commit fail closed"

new_case
printf 'newer\n' >> "$repository/content.txt"
"$real_git" -C "$repository" commit -q -am newer
newer_revision=$("$real_git" -C "$repository" rev-parse HEAD)
printf 'newer release\n' > "$case_dir/newer.md"
seed_annotated_tag v8.0.0 "$newer_revision" "$case_dir/newer.md"
if run_plan patch; then fail "a superseded untagged run was released"; fi
assert_contains "$stderr" 'was superseded by the already-released v8.0.0 commit'
pass "an older untagged run cannot publish after a newer release"

new_case
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v9.0.0 "$base_revision" "$case_dir/predecessor.md"
run_plan minor || fail "the conflicting-target fixture was not planned"
expected="$case_dir/expected.md"
mv "$output" "$expected"
seed_annotated_tag v9.1.0 "$base_revision" "$case_dir/predecessor.md"
"$real_git" -C "$repository" tag -d v9.1.0 >/dev/null
"$real_git" -C "$repository" tag -a --cleanup=verbatim \
  v9.1.0 "$full_revision" -F "$expected"
: > "$github_output"
if run_plan minor; then fail "a remote target conflict was accepted"; fi
assert_contains "$stderr" 'instead of'
pass "a current local tag that conflicts with the remote target fails closed"

new_case
tree=$("$real_git" -C "$repository" rev-parse "${base_revision}^{tree}")
divergent_revision=$(printf 'divergent\n' \
  | "$real_git" -C "$repository" commit-tree "$tree")
printf 'predecessor\n' > "$case_dir/predecessor.md"
seed_annotated_tag v10.0.0 "$divergent_revision" "$case_dir/predecessor.md"
seed_annotated_tag v10.1.0 "$full_revision" "$case_dir/predecessor.md"
if run_plan minor; then fail "a divergent numeric predecessor was accepted"; fi
assert_contains "$stderr" 'is not descended from its numeric predecessor v10.0.0'
pass "a current tag cannot use a divergent numeric predecessor as its bump base"

printf '1..%d\n' "$tests"
