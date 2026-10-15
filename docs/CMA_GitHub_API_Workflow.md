# GitHub API source workflow

The CMA Agent can prepare a useful local source workspace through `gh api` with
the existing `api-github` grant. Download the default branch when the user did
not select a ref. This work is performed by the Agent's ordinary tools; CMA does
not add a repository downloader or change NyxID.

## Resolve one source snapshot

Use the repository selected by the user. Validate its owner/name, and treat any
ref or path as data rather than shell input. Quote shell values and URL-encode
ref/path components, including a branch name containing `/`. Never interpolate
repository text into executable shell syntax. Keep the runtime's `gh` config
and environment intact.

1. Read `GET /repos/{owner}/{repo}` through `gh api`. When no ref was supplied,
   select `default_branch` from this response. Honor an explicit branch, tag or
   commit; never silently substitute the default branch for a failed ref.
2. Read `GET /repos/{owner}/{repo}/commits/{ref}`. Retain the returned commit
   `.sha` and `.commit.tree.sha`. Subsequent reads must use these immutable
   object IDs, even if the branch advances during the download.
3. Download `GET /repos/{owner}/{repo}/tarball/{sha}` to a fresh temporary file,
   for example `gh api --method GET "repos/$owner/$repo/tarball/$commit_sha" > "$archive"`
   after safely resolving those values. Check the command's exit status and
   validate gzip/tar bytes before inspecting or extracting the archive. Treat
   an error JSON body or partial download as a failure, never as source.
4. Validate and stage the archive as described below, then reconcile its files
   with the current destination. A single archive request avoids a separate API
   call for every source file.

The API archive endpoint redirects to `codeload.github.com`, and the existing
NyxID server can follow that redirect internally. On 2026-10-15, a read-only
`nyxid proxy request api-github repos/ChronoAIProject/cma/tarball/develop --stream`
probe returned a valid 7,688,879-byte gzip tar with 4,703 entries and archive root
`ChronoAIProject-cma-bd072483a7dd96260ca6587ec97ca7697de2bdcf`. Its SHA-256 was
`cc73373ea42b81acb7c9c84c76ab4c2edc23955a16712611e471be2f6238ab21`.
This establishes that endpoint's existing NyxID archive behavior without
credential extraction or a service change. It is distinct from an Agent's
runtime `gh` acceptance, which must use its own authorized Sandbox scope.
The CMA client's redirect policy alone does not determine whether NyxID follows
the upstream redirect before returning the final bytes.

If the archive is unavailable, read the pinned commit's
`GET /repos/{owner}/{repo}/git/trees/{tree_sha}?recursive=1`. Explicit GET matters
when using `gh api -f recursive=1`, because field flags otherwise select POST.
If `truncated` is true, walk nonrecursive subtrees and join child paths under
their parents; do not treat a partial recursive response as a full checkout.
Read ordinary files from `GET /repos/{owner}/{repo}/git/blobs/{blob_sha}` and
decode base64 as bytes, preserving binary data and exact newlines. Verify size
and Git blob object ID before publication. Cache repeated blobs and bound
concurrency. Contents at the pinned SHA is also suitable for a small selected
set of files; do not assume its directory listing covers arbitrarily large
trees or that a returned `download_url` is reachable through this API transport.

If an API size limit, rate limit or inaccessible object interrupts a download,
retain staging and report what is incomplete; do not call a partial checkout
ready. Missing GitHub service access uses `agent_key_scope_revision` for
`api-github`, followed by CMA's confirmed update before retrying. Repository or
organization permission errors may remain after service access is granted.

## Preserve the local workspace

Inspect the entire archive member list before extraction, enforce limits on its
entry count and expanded size, and strip only the common top-level directory.
Reject device/FIFO/special entries and hardlinks. Do not invoke an unchecked
extract-all operation on repository-controlled paths. Stage source in a fresh
directory and inspect the full path/type manifest before placing it in the
destination. Reject absolute paths, empty/dot/traversal
components, `.git` entries and any path that escapes the chosen root. Check
existing parent directories without following symlinks. Reject file/directory
collisions and unexpected object modes. Existing files, untracked files, dirty
changes and Git metadata must survive; reconcile differences before replacing
anything, and do not infer deletions from a partial listing.

Tree modes `100644` and `100755` identify ordinary and executable files. Preserve
those permissions without adding privileged bits. Mode `120000` is a symlink:
read its target blob, allow only a deliberate target that stays within the source
root, and create symlinks after ordinary file writes so later writes cannot
follow them. Report any symlink that cannot safely be materialized. Mode `160000`
is a submodule commit, not a downloadable file or directory; it requires a
separate explicitly selected repository/ref and its permissions. Git LFS pointer
blobs are pointers, not the large object contents. Report unavailable submodule
or LFS content; do not invent their contents or fetch arbitrary embedded URLs.

Use the current CMA Git worktree when it already exists. Check whether the
destination is inside a Git repository; run local `git init` only when it is not.
Never replace an existing `.git` file/directory or run a reset/clean to make the
download fit. Local editing, diffs and commits can use ordinary Git. Record the
source repository, selected ref and commit SHA in the task result; a local
snapshot commit is not the upstream commit and must not claim its identity.
Remote history is not needed for local source work. Report the result naturally
as source checked out at the selected commit, without claiming native remote
clone, pull or push support.

## Create repositories and publish requested changes

Create a repository only when the user's task requests it. For the authenticated
user, use `gh api --method POST user/repos --input request.json`; for a selected
organization, use `POST /orgs/{org}/repos`. Build the JSON file with a serializer,
including the intended name and explicit `private` boolean. Verify the returned
owner, name and visibility. If the request result is uncertain, read the exact
repository before retrying rather than creating a differently named duplicate.
An empty repository has no branch commit to resolve; local `git init` remains
valid, and a requested first file can initialize the default branch through the
Contents API.

For one-file changes, the Contents API accepts base64 bytes, the target branch
and the observed file SHA for updates/deletions. For an atomic multi-file change,
read the intended branch ref and parent commit, create the needed blobs, create
a tree with the observed `base_tree`, and create a commit whose parent is the
observed branch tip. Preserve unmodified paths and modes; encode deletions
explicitly. Create a new branch through `POST /repos/{owner}/{repo}/git/refs`, or
update an existing branch through `PATCH .../git/refs/heads/{branch}` with
`force: false`. Re-read the ref immediately before publication and reconcile a
changed tip. A non-fast-forward refusal must trigger another read/reconciliation,
not a forced retry. Verify the returned/read-back ref points to the intended
commit; protected branches or organization policies may require a new branch
and pull request instead.

These are GitHub API writes with the user's requested scope. They do not
authenticate native Git network commands. Do not extract tokens, change the
runtime's `gh` auth/configuration, embed credentials in Git remotes, or ask the
user to paste a secret.

## Runtime verification on 15 October 2026

A disposable CMA Agent used the installed `gh api` against the existing NyxID
connection to resolve the private `ChronoAIProject/cma` default branch `develop`
to `bd072483a7dd96260ca6587ec97ca7697de2bdcf`, download its tarball, validate and
extract it, and initialize a local Git workspace. The 7,688,879-byte archive has
SHA-256 `cc73373ea42b81acb7c9c84c76ab4c2edc23955a16712611e471be2f6238ab21` and
4,703 archive entries. An independent read of the same Pod confirmed that hash,
README presence, `git rev-parse --is-inside-work-tree=true`, and branch `develop`.
The test did not run repository code or perform a native Git network operation.
This verifies the source workflow on the current runtime; new persistent system
instructions require a separate new-Agent check after their API deployment.
