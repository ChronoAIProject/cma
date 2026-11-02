# AI Task Workflow

GitHub Issues are the single source of task state for CMA. A local branch, a worktree, a chat message, a stale document or an unmerged PR cannot replace the Issue's current status and acceptance evidence.

## Select a task

1. List open Issues and select an Issue carrying the repository's existing `status:ready` label. Read the complete body, comments, linked PRs and related Issues.
2. Confirm that the Issue describes one independently reviewable result. Its acceptance checklist must cover the problem, evidence, scope, verification, dependencies, delivery boundary and required permissions.
3. Check that the Issue is not already `in-progress`, `blocked`, assigned through an active branch/PR, or covered by a merged change awaiting deployment acceptance. Search by key terms before claiming it.
4. If any required fact is missing, leave the Issue open without `status:ready` and record the missing fact or blocker. Do not turn an old report, TODO or code preference into a ready task.

The current task pool and evidence boundary are in [`PROJECT_STATE.md`](PROJECT_STATE.md). The linked Issue remains authoritative when the document and Issue differ.

## Check conflicts and dependencies

Before editing, fetch the target baseline and inspect all local work:

```sh
git fetch origin develop --prune
git status --short
git worktree list
gh issue view <number> --repo ChronoAIProject/cma
gh pr list --repo ChronoAIProject/cma --state open
```

Never reset, clean, stash, amend or overwrite a dirty worktree that was not created for the task. If a dirty file overlaps the Issue, stop and report the conflict or use a new worktree. A branch name, open PR or current diff is evidence of active work; do not start a duplicate implementation.

Check every dependency named by the Issue. External API behavior, a deployment image, an owner authorization, a production fixture or a missing credential is a real prerequisite. If it is unavailable, add the exact missing condition and its release condition to the Issue, apply `blocked`, and stop implementation.

## Claim and implement

Claim only after the checks pass. Use a clean worktree and the repository branch convention:

```sh
git worktree add /path/to/worktree -b codex/<issue-number>-<short-name> origin/develop
```

Add `in-progress` to the Issue and comment with the branch or PR URL. Preserve the existing labels and assignments. Keep the implementation within the Issue's scope; put implementation, tests and documentation in one Issue checklist rather than creating step Issues.

Read `CLAUDE.md` and any deeper applicable instructions before editing. Keep each file in its owning layer, update `infra/` when deployment surface changes, and keep public interface claims tied to evidence. Do not add a root entry or weaken a CI gate.

## Verify and submit

Start with the narrowest meaningful checks for the changed behavior, then run the repository checks required by the Issue and its affected layer. Distinguish these results:

- source/unit or contract tests prove local behavior;
- image and manifest checks prove build/deployment consistency;
- a deployed environment check proves the named deployment and its exact revision;
- owner or external-provider acceptance proves the authorization and provider boundary.

Do not describe one category as another. If a test is unavailable, state why and leave the corresponding acceptance item open.

Open one PR into `develop` with the Issue linked. The PR description must state the resulting behavior, the evidence used, the checks run and any remaining deployment or provider acceptance. Add the PR URL to the Issue. Keep the Issue `in-progress` until its stated delivery boundary is met.

## Stop and report

Stop implementation and update the Issue when:

- the current behavior cannot be reproduced or the reported root cause is not established;
- another branch, PR, dirty worktree or session owns overlapping work;
- a dependency requires an unavailable API contract, credential, deployment, external service or owner decision;
- validation would mutate production or a user-owned resource without the Issue's explicit acceptance boundary;
- a test fails for an unrelated pre-existing reason and fixing it would expand scope;
- the proposed fix would change a public contract, deployment surface or persisted schema without the required compatibility evidence.

Record the observed command/result, the blocker, and the exact condition that unblocks it. Do not label a task ready merely because the blocker is inconvenient.

## Update state

- Leave an unprepared candidate open without `status:ready`.
- Use `status:ready` only when another AI can begin from the Issue without guessing.
- Use `in-progress` only with an explicit claimant and a recorded branch or PR.
- Use `blocked` with a concrete blocker and an unblocking condition.
- Close only after every Issue acceptance item and its delivery boundary are satisfied. Merging code is not deployment acceptance unless the Issue explicitly says the result ends at merge.

After a meaningful state change, update the Issue first, then update [`PROJECT_STATE.md`](PROJECT_STATE.md) with the evidence link. Keep the project document as a short evidence map; do not duplicate a full Issue task table there.
