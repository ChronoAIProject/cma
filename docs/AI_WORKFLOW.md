# AI Task Workflow

GitHub Issues hold CMA task scope, ownership, decisions and acceptance evidence.
A branch, worktree, chat report or unmerged PR is evidence of work, not a completed
delivery. Use [PROJECT_STATE.md](PROJECT_STATE.md) to find the current coordination
records; the linked issue remains authoritative.

## Select and understand the task

1. Start from an explicit assignment or an issue carrying the existing
   `status:ready` label. Read its complete body, comments, linked PRs, dependencies
   and acceptance checklist. A label does not override a newer blocker.
2. Identify one independently reviewable result, the affected layer, the required
   permissions, and where delivery ends: repository documentation, merged code,
   deployment, production verification, or a measured qualification window.
3. Search existing issues and open or merged PRs before implementing. A merged
   change awaiting deployment acceptance needs verification, not a duplicate fix.
4. Record missing facts and the exact condition that resolves them. Do not infer
   production state, available capacity or authorization from an old task list.

Read [CLAUDE.md](../CLAUDE.md) and any applicable directory instructions before
editing. Keep changes in their owning layer and update `infra/` when deployment
surface changes. Existing runbooks and interface contracts remain authoritative;
this intake guide does not replace them.

## Check ownership and workspace state

Verify the authorized GitHub account before GitHub or git work. Do not switch a
shared account to resolve an identity mismatch; report it to the task lead.

```sh
gh auth status --active
git status --short --branch
git worktree list
gh issue view <number> --repo ChronoAIProject/cma --comments
gh pr list --repo ChronoAIProject/cma --state open
git fetch origin
```

Reuse the task's dedicated worktree when one is already assigned. Otherwise use
an isolated worktree and the branch convention specified by the assignment,
based on the current integration branch. Do not enter, clean, reset, stash or
rewrite another task's worktree. Inspect overlapping branches and PRs; coordinate
ownership on the issue before making conflicting edits.

Comment on the issue with the accepted scope, branch or existing PR, dependencies
and delivery boundary. Preserve unrelated labels and assignments. Use the
repository's existing `in-progress`, `blocked` and `status:ready` labels when
appropriate; do not treat a proposed owner as confirmed capacity.

## Implement and verify

Keep commits single-purpose and explain why the change is needed. Reuse existing
contracts and implementations; include meaningful success and failure checks for
changed behavior. Run the affected checks required by the issue and repository,
respect shared build limits, and leave full CI gates enabled. For documentation,
check the changed links, claims, structure, file sizes, whitespace and secrets;
do not describe unrun runtime tests as passing.

Keep the following evidence separate:

| Evidence | What it establishes |
|---|---|
| Source, unit, integration or contract checks | Behavior under the stated local or CI conditions |
| Image, manifest and asset checks | Identity and consistency of the tested deployment artifact |
| Deployed observation | Behavior of the named environment, revision and configuration |
| Owner/provider acceptance | The specific permission or external contract that was verified |
| Calendar-bound qualification | Only the elapsed, complete window required by the gate |

For runtime acceptance, record source SHA, CI run/attempt, image digests,
configuration/flags, schema and reader floor, environment, time window, sample
count and sanitized raw evidence. Keep first-attempt failures and distinguish
PASS, FAIL, BLOCKED and NOT_RUN. Cancelled, skipped, missing or retry-only evidence
does not qualify as a first-attempt pass.

An unavailable dependency blocks its dependent action. Continue independent,
authorized work and record the blocker, responsible role and release condition.
Do not invent credentials, targets, fixtures, provider behavior or measurements.
Production mutation, data deletion and policy changes require their explicit
scope and authorization; a documentation draft does not provide it.

## Submit, review and release

Reuse an existing valid PR for the task; otherwise open one logical PR into
`develop` after the required local checks. Link the issue with `Closes #N` only
when the PR resolves its full delivery scope. Use `Part of #N` or `Refs #N` when
production acceptance, external decisions or other implementation remains.
State the final behavior, validation, evidence limits and remaining dependencies
in the PR body, and link the PR from the issue.

Before a push, fetch and merge the current integration branch into the task
branch. Preserve published history: no rebase or force push. Update only the PR
assigned to this task and follow the active coordination policy for pushes and
re-review. Record the new head when the reviewer needs a delta review.

Follow [CLAUDE.md](../CLAUDE.md) and [Release.md](Release.md) for normal merge and
release rules. Any temporary sprint exception must be explicitly authorized for
the PR and retain independent review, the required validation and exact commit
matching. Restore and verify any temporarily changed protection immediately.
A merge exception does not authorize deployment or promotion of a red artifact.

Deployment belongs to the designated release owner. Use the approved artifact
and preserve retained assets, sessions, reader compatibility and rollback
constraints. Implementation teams attach the relayed production evidence to the
issue; a merged PR alone does not establish production acceptance.

## Maintain the evidence map

Update the issue when work starts, its PR is ready, it merges, and its required
verification completes. Record scope changes and decisions with dated evidence;
do not overwrite a failed observation with a later successful attempt.

Close only when the issue's acceptance and delivery boundary are met. A completed
report can close while its linked engineering work remains open. A policy draft
awaiting confirmation or an unfinished qualification window remains open.

Keep [PROJECT_STATE.md](PROJECT_STATE.md) as a short index of authoritative
coordination and evidence records. Update links when responsibility moves; keep
live assignments, decisions and checklists in GitHub rather than duplicating a
backlog or naming a permanent "next task" in repository documentation.
