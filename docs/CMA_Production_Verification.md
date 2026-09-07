# Production Trigger Verification

## Current Acceptance Campaign

The later 2026-09-07 Truretruing request supersedes the historical runtime-change
restriction below. It explicitly authorizes Autonomous agents to use YOLO without
user approvals, runtime replacement, and targeted production-state cleanup. Work
is built from `feat/trureturing-trigger-loop-20260907`; no aggregate branch merge
is needed for deployment. Earlier observations remain historical evidence, not
acceptance of this campaign.

| Requirement | Required Production Evidence | Status |
| --- | --- | --- |
| 1. Public Agent Profile | Exact text, model, effort, Autonomous, four latest public skill pins | Catalog and archive checksums verified; creation pending |
| 2. Public Workspace Profile | Exact metadata, repository `dev`, no setup, one Agent Profile | Configuration prepared; creation pending |
| 3. Public Sandbox Profile | 4000 millicores, 8192 MiB, two requested services, 4/4 limits, working toolchain | Pinned .NET/Lean installers prepared; runtime proof pending |
| 4. Public Trigger | Exact title/message, all-level reuse, Try It, visitor theme, no review, README placement | Configuration prepared; creation pending |
| 5. Login without bootstrap | No Sandbox, Workspace, Agent, or Profile created by login alone | Regression and production verification pending |
| 6. Complete external-click flow | Login then automatic provisioning; exact first message once; correct software, skills, scopes | Production browser verification pending |
| 7. Previous UI style | Original rotating 3D mark and established controls; desktop/mobile screenshots | Restoration in progress |
| 8. Autonomous YOLO | Pinned provider runs full-access mode with no approval requests | Provider correction in progress |
| 9. Lifecycle | Active to paused to active; paused to terminated; history retained | Checkpoint correction and production exercise pending |
| 10. Cleanup | Only identified stale state removed; new published flow remains usable | No campaign state deleted yet |
| 11. Conversation UI | Thinking, Send/Stop, CPU, RAM, context, token, network metrics | Restoration in progress |

Private campaign evidence is kept under `/tmp/cma-trureturing-loop.LvTezb` and
must not be committed or uploaded wholesale. One existing NyxID owner account is
available; fresh signup and cross-owner browser isolation are not represented as
tested by reusing that account.

## Historical Observation

Recorded on 2026-09-07, Singapore time. This is a bounded production observation,
not a complete refactor acceptance or an unattended end-to-end pass.

## Deployed Components

The image digests in `infra/api/deployment.yaml`, `infra/web/deployment.yaml`, and
`infra/api/configmap.yaml` match the observed production pins. Recording them
does not apply manifests or change the live runtime.

| Component | Source or State |
| --- | --- |
| API | `8a2272e872dd87c0063294ca967d15089fb3f9e0` |
| Web | `23987739234a763ff3826e1c819dc56efc5d68d5` |
| Runtime | Existing contract-3 image retained; Codex 0.147.0 and ACP 1.4.0 |

## Trigger Evidence

The original Trigger remains at revision 3 with its original revision-1 profile
pins. Its exact first message is `Shinin Test`; its spelling was not changed.
The README badge was merged through PR #840. GitHub's authenticated rendered
README response contains the correct link and image; the Camo image returned
HTTP 200. A signed-in GitHub browser click was not automated.

The fresh-provisioning test used a separate Trigger with the same profile pins
and first message, forcing a new Sandbox. It exercised the anonymous landing,
the real CMA/NyxID OAuth callback using the existing owner's CLI credential,
explicit review consent, durable provisioning, and first-message execution.
This does not cover a new NyxID signup or another owner's account.

| Check | Observed Result |
| --- | --- |
| Fresh Sandbox and Workspace provisioning | Completed |
| Profile Agent and separate Trigger Agent/worktree | Both present with distinct worktrees |
| First message | Exactly one `Shinin Test` in the test Agent's first turn |
| First response | Completed after individually inspected read approvals |
| Applied configuration | Model, effort, permission policy, and system-prompt hash match the pinned Agent Profile |
| Skill installation | Selected skill exists in the Agent-local home |
| Git and processes | Worktree root, branch, base commit, and process working directories match |
| Browser return | Completed transcript preserved; message input enabled |
| Workspace expansion | Child Agents visible and the completed Agent navigable |
| Desktop and mobile | Final response visible without input overlap at 1440px and 390px |

The fresh test first stopped because its approval allowlist expected `wc -l`
instead of the actual `wc -l CLAUDE.md`. After correcting that exact test check,
the first response completed at 21:47:40. The continuation harness then rejected
the resource count because concurrent owner activity had added a third Agent.
Neither attempt is relabeled as a passing end-to-end run. A separate read-only
check verified the completed turn and UI, allowing additional owner resources
while still requiring the two known, distinct test worktrees.

The owner's actual original Trigger launch at 21:46:53 recorded the GitHub
referrer, review consent, reuse of the fresh Sandbox and Workspace, creation of
another distinct Agent/worktree, and delivery of the exact first message. Its
Agent was still waiting for a read approval at the final read-only observation.
This is observed production state, not an independently automated GitHub click.

## Blocking Findings

1. Ordinary commands fail inside the pinned provider's inner Linux sandbox with
   `bwrap: loopback: Failed RTM_NEWADDR: No child processes`. The autonomous
   profile correctly maps to `on-failure` and `workspace-write`, but this failure
   causes repeated manual approval requests. A completed manually approved turn
   does not demonstrate a smooth unattended Trigger journey. No sandbox bypass,
   full-access policy, network expansion, or runtime upgrade was applied.
2. The original Sandbox entered `failed` during inactivity pause at 20:40:20,
   before the latest API rollout, reporting `restore_failed` and a rejected
   lifecycle operation. Its runtime remained Running with zero restarts. A
   likely checkpoint blocker is the default `.codex/tmp/arg0` executable-alias
   tree: its links leave the home, while current provider exclusions cover only
   Agent-local alias roots. This is a candidate cause, not a captured runtime
   rejection detail. No original-home modification, restart, database repair,
   or forced recovery was attempted. Existing completed transcripts and
   worktrees were verified preserved after rollout.

The official OpenAI [sandbox and approval documentation](https://learn.chatgpt.com/docs/agent-approvals-security)
describes Linux `bwrap`/`seccomp` enforcement and container compatibility limits.
It does not establish compatibility of this pinned runtime with this cluster.
Any remediation still needs evidence against the actual isolation boundary;
disabling that boundary is not a verified fix.

## Remaining Acceptance

- Resolve provider/runtime compatibility with preserved permission boundaries
  on isolated, newly created test Sandboxes; runtime-change scope remains
  unapproved and the production runtime image must remain unchanged.
- Resolve failed-pause recovery without losing or interrupting the original
  Sandbox's work, then prove supported pause/restore and continued conversation.
- Repeat the exact README Trigger journey without manual approval workarounds.
- Complete broader owner/lifecycle verification on dedicated test resources.
- Keep fresh-signup, cross-owner isolation, and signed-in GitHub browser coverage
  explicitly unverified until the required account/browser context exists.

The fresh Trigger Sandbox now contains concurrent owner activity. It is no
longer exclusively test-owned and must not be paused, stopped, or cleaned up by
the test harness. The original Trigger, profiles, both live homes, and histories
were retained. No `develop`/`main` merge or CI bypass was performed in this step.

Detailed local evidence is retained in
`/tmp/cma-delivery-8c8f70d7.1TkISv/trigger-fresh-8a2272e8-first/` and
`/tmp/cma-delivery-8c8f70d7.1TkISv/trigger-fresh-8a2272e8-completed-readback/`.
These directories contain private browser state and must not be uploaded or
committed wholesale.
