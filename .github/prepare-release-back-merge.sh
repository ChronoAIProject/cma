#!/usr/bin/env bash
# Prepare the history-only PR branch using the publisher's contents permission.
set -euo pipefail

refuse() {
  printf '::error::%s\n' "$1" >&2
  exit 1
}

release=${1:-}
[[ "$release" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" = "$release" ]] \
  || refuse 'The back-merge checkout does not match the release commit.'

git fetch --quiet --no-tags origin \
  '+refs/heads/develop:refs/remotes/origin/develop' '+refs/heads/main:refs/remotes/origin/main'
develop=$(git rev-parse --verify 'refs/remotes/origin/develop^{commit}')
main=$(git rev-parse --verify 'refs/remotes/origin/main^{commit}')
if [[ "$release" != "$main" ]]; then
  git merge-base --is-ancestor "$release" "$main" || refuse 'The release commit is outside the current main history.'
  echo 'A newer main commit owns the back-merge handoff.'
  exit 0
fi
if git merge-base --is-ancestor "$release" "$develop"; then
  echo 'Develop already contains the release commit.'
  exit 0
fi

develop_tree=$(git rev-parse "$develop^{tree}")
merge_tree=$(git merge-tree --write-tree --no-messages "$develop" "$release" 2>/dev/null) \
  || refuse 'The release back-merge needs conflict resolution on a reviewed branch.'
[[ "$merge_tree" = "$develop_tree" ]] \
  || refuse 'The release back-merge would change develop content. Review and port the differences before reconciling history.'

branch="chore/back-merge-main-$release"
ref="refs/heads/$branch"
advertised=$(git ls-remote --refs origin "$ref")
if [[ -n "$advertised" ]]; then
  git fetch --quiet --no-tags origin "$ref"
  parents=$(git rev-list --parents -n 1 FETCH_HEAD)
  read -r candidate first second extra <<< "$parents"
  [[ -n "$first" && "$second" = "$release" && -z "$extra" ]] \
    && git merge-base --is-ancestor "$first" "$develop" \
    && [[ "$(git rev-parse "$candidate^{tree}")" = "$(git rev-parse "$first^{tree}")" ]] \
    || refuse 'The published back-merge branch no longer matches the history-only handoff.'
  existing_merge=$(git merge-tree --write-tree --no-messages "$develop" "$candidate" 2>/dev/null) \
    || refuse 'The published back-merge branch cannot merge cleanly into develop.'
  [[ "$existing_merge" = "$develop_tree" ]] \
    || refuse 'The published back-merge branch would change develop content.'
else
  candidate=$(git commit-tree "$develop_tree" -p "$develop" -p "$release" \
    -m 'Merge the published release into develop')
  git push origin "$candidate:$ref"
fi

compare="${GITHUB_SERVER_URL:?}/${GITHUB_REPOSITORY:?}/compare/develop...$branch?expand=1"
{
  printf '### Release back-merge\n\n'
  printf 'Verified history-only branch tip `%s` for release commit `%s`.\n\n' "$candidate" "$release"
  printf '[Open the back-merge pull request](%s) into `develop` and merge it using a merge commit.\n' "$compare"
} | tee -a "${GITHUB_STEP_SUMMARY:?}"
