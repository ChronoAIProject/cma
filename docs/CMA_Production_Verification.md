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
| 1. Public Agent Profile | Exact text, model, effort, Autonomous, four latest public skill pins | Created and published at revision 1; exact configuration and archive checksums verified |
| 2. Public Workspace Profile | Exact metadata, repository `dev`, no setup, one Agent Profile | Created and published at revision 1; exact configuration verified |
| 3. Public Sandbox Profile | 4000 millicores, 8192 MiB, two requested services, 4/4 limits, working toolchain | Exact configuration verified in production; actual Agent workload exposed a login-shell PATH defect; corrected-image workload pending |
| 4. Public Trigger | Exact title/message, all-level reuse, Try It, visitor theme, no review, README placement | Exact revision published after first rollout; README-only PR #841 merged after all 12 protected checks; default README snippet read back exactly |
| 5. Login without bootstrap | No Sandbox, Workspace, Agent, or Profile created by login alone | Existing-owner fresh-browser production sign-in created no resources before or after rollout; fresh signup not tested |
| 6. Complete external-click flow | Login then automatic provisioning; exact first message once; correct software, skills, scopes | Corrected production first click and repeat reuse passed with no extra clicks; exact skills/scopes verified; actual workload and final-image rerun pending |
| 7. Previous UI style | Original rotating 3D mark and established controls; desktop/mobile screenshots | Live home and provisioning animation passed in desktop/mobile, both themes and reduced motion; final chat corrections awaiting rollout |
| 8. Autonomous YOLO | Pinned provider runs full-access mode with no approval requests | Actual provider configuration verified; first response and subsequent workload attempt completed with zero approvals |
| 9. Lifecycle | Active to paused to active; paused to terminated; history retained | Compressed periodic production checkpoints published; heavy built-home pause/restore and final runtime rotation still pending |
| 10. Cleanup | Only identified stale state removed; new published flow remains usable | Two old Sandboxes retired through the public API under explicit authorization; final environment pending |
| 11. Conversation UI | Thinking, Send/Stop, CPU, RAM, context, token, network metrics | Live observations found missing gVisor limits, clipping and follow defects; exact configured-capacity fallback and layout fixes tested locally; production acceptance pending |

Private campaign evidence is kept under `/tmp/cma-trureturing-loop.LvTezb` and
must not be committed or uploaded wholesale. One existing NyxID owner account is
available; fresh signup and cross-owner browser isolation are not represented as
tested by reusing that account.

The exact profile pins are Agent `agp_5119efbbf5fe444db826efcb7b347e54`,
Workspace `wkp_0720afc39fba43e1a66e0d8ce56d2ab9`, and Sandbox
`sbp_3310b0ac32ee4ad495e77e6412924676`, all revision 1. Trigger draft
`trg_b818aaafa3284e8495faed7eefe27219` uses placement
`plc_08e81ca7da574d86a8addfb61d550ecd`.

The isolated Linux runtime installed .NET 10.0.103, Elan 4.2.4 and Lean/Lake
4.33.0 as UID 10001 with the requested 4 CPU / 8 GiB resources. Against repository
`dev` commit `63ba7cc8376be34ff5ba4ed3bb73c0eab56f1f6d` in a separate worktree,
the .NET build, 16 fast checks, selftest and the 890-job Lean conventions build
passed. Codex 0.147.0 discovered each of the four selected skill archives once.
This is executable compatibility evidence, not production acceptance. The full
workload's 346,244 entries and 85.16 MiB manifest required raising checkpoint
metadata limits to 500,000 entries / 128 MiB; production pause/restore remains
unverified until that runtime is deployed and exercised.

### First Live Attempt

At 2026-09-07 15:45 UTC, API `d4869f15`, web `d55e581e`, and runtime `36f7e8f5`
were deployed and became ready. The exact Trigger was then published as
`pub_d77caf24c76f4ce188e569a83c71b8a1`. README-only PR #841 uses its published
snippet, without merging the implementation branch into `develop`.

A real browser click on GitHub-rendered README HTML delivered at the source
origin completed NyxID sign-in and admitted one Launch without additional CMA
clicks. This method does not claim authenticated access to the GitHub web page.
Launch `lch_cb061b4385e3470495cbf30faef521dc` created Sandbox
`sbx_2990e2353c61408a86b45f70d4544140`, completed all five software commands,
activated the exact two-service NyxID key, and cloned `trureturing` `dev` into
one Workspace. The repository had advanced to commit
`65457467c879007d1e91f71ef861a4d3f7d462c3` since the isolated toolchain test.

The attempt failed before Agent creation. An immediate periodic checkpoint
treated the Workspace as quiet before its configured Agent had been admitted,
then fenced creation. The runtime sealed its 4,000,983,562-byte home archive, but
sequential 32 MiB upload/readback exceeded the 600-second upload lifetime. Launch
dependency retries were exhausted while this legitimate writer hold remained.
No first prompt ran. The failing browser evidence is retained in
`production-first/`; it must not be relabeled as a passing end-to-end run.

The configured-Agent-before-checkpoint correction is committed in `e132f0a2`,
with real Mongo regression evidence in `9fa6cff7`. Commit `380f0457` preserves
durable Launch waiting and creation identity during checkpoint admission races;
`dfe1d7ce` transfers four ordered parts concurrently with bounded buffers and
verified readback. These corrections have passed focused core, Mongo, worker,
and storage regression tests and are building for the next rollout.
Retirement was requested through the public API for this identified test
Sandbox, which had zero Agents and zero Worktrees. No database repair was used.

Live production browser evidence verifies the original green/yellow 3D marks,
still states, four desktop/mobile theme layouts, and the absence of creation
during login alone. Running-step motion and a completed Agent conversation are
still pending the corrected production run.

### Corrected Run Preparation

README-only PR #841 merged normally as `chronoai-shining` at
2026-09-07 16:24:09 UTC after all 12 required checks passed. Merge commit
`cf219c59be5cfeb95c21eaaad66d6e5bb6cd704c` changes only the README button.
The default README blob `bb7196be076502da99c6652f019e1d2d6550d74e` was read
back and matches the published Trigger's Markdown snippet exactly.

Cluster-local storage measurements gave 6.547 seconds for a single 32 MiB
upload plus verified readback, and 11.757 seconds for four simultaneous parts.
All five temporary measurement objects were deleted and verified absent. This
improves the initial 4 GB transfer but does not by itself fit the fully built
13.7 GB home within the existing 600-second object upload lifetime.

Runtime source `0412d647` therefore streams deterministic gzip checkpoints and
restores both plain and compressed archives with the existing path, file-hash,
entry, metadata, and uncompressed-payload bounds. Its focused corruption,
truncation, trailing-member, deterministic-replay, and legacy-restore tests pass;
the full runtime suite has 161 passing tests and one external-ZIP test ignored.
Production compression, transfer duration, and full-home restore still require
the corrected live run. Adoption is forward-only relative to plain-only runtime
images, as recorded in `Release.md`.

The repository workload verifier sends one real Agent message through the
deployed chat UI. It requires the .NET build, fast checks, selftest, and Lean
conventions target to record successful process exit codes in a local receipt.
A separate read-only check validates that receipt, the actual worktree, and
unchanged tracked source. The receipt check must pass before an Agent's success
message can count as toolchain acceptance. Repeat-Trigger verification compares
the entire existing conversation and requires the first prompt exactly once,
including after legitimate follow-up work.

### Corrected Production Journey

API `380f0457`, web `d55e581e`, and runtime `0412d647` completed the corrected
production entry. Launch `lch_8c440c2ae7b54609852d24edaa76e112` provisioned
Sandbox `sbx_e5bb9c67ceb747e29f6d71faab0c6f97`, Workspace
`wks_23ab9751dc9c4459bb8e132926abd8f0`, Worktree
`wkt_8dcc607cd4e247088bd4210e30f8a716`, and Agent
`agt_9b174a0ff4d34a9ea507caa92f9847a0`. Its exact first prompt completed at
2026-09-07 17:00:21 UTC with zero extra CMA clicks and zero approval requests.
The elapsed click-to-answer time was approximately eleven minutes, including
provisioning and the model's tool use; this is not a low-latency claim.

Repeat Launch `lch_ed34f1aef68d495a9f037ca0957724b1` reused all four instance
identifiers and preserved the complete conversation with the first prompt
exactly once. Evidence remains in `production-first-fixed/` and
`production-repeat/`. Both used GitHub-rendered README HTML at the source
origin and a genuine browser anchor click, not authenticated GitHub page access.

A read-only configuration probe while the Agent was working verified all five
installers, profile environment and instructions, `gpt-5.6-sol` with `xhigh`,
`never` and `danger-full-access`, twenty exact skill files across four archives,
native skill discovery once each, the separate Agent home and worktree, and the
NyxID key's exact `chrono-llm,ornn-api` service scope with `proxy read` and no
node/all grants. The worktree was `/home/cma/trureturing/.worktrees/wt-1`, branch
`cma/wt-1`, base `c382165c4623d22e1f59d0e7412b1b3183f42217`, with clean tracked
source. This probe did not yet reproduce the Agent's login-shell environment.

### Actual Workload Failure And Follow-Up

The real Agent workload attempt `workload-toolchain/` failed honestly:
`make -C tools dotnet` exited 2 because `dotnet` was not found. The provider
process inherited the configured PATH, but Debian `/etc/profile` reset it inside
`bash -lc`; the separate Agent home lacked the main home's restoring profile.
The failed receipt and attempt are retained and never count as acceptance.

Runtime `8ebb0db9` preserves nonempty inherited PATH in the image's login-shell
initialization, without editing live Agent homes. Its regression reproduces the
distinct Agent HOME, login/non-login shells, tool directories with spaces,
empty/unset fallback and intentional owner-profile overrides. The previously
deployed runtime fails the new login-shell check. Actual fixed-image execution
and receipt readback remain required after rollout.

API `f88d7cf0` renews exact periodic capture claims, bounds total capture and
metadata recovery, and uses revision-fenced retry after ambiguous publication
or timeout without releasing an uncertain writer hold. Its follow-up tests
`7b15a8b6` passed all fourteen real-Mongo cases, including renewal/takeover races
and an ambiguous-commit failpoint. Only the disposable fixture was changed.

The exact compressed-runtime full-home fixture sealed 13,799,459,617 raw bytes
into 6,175,626,862 gzip bytes in 233.150 seconds. Exact hold replay, downloaded
digest and writer resume passed. Heavy production upload and full-home restore
remain unverified: extrapolated transfer time is close to the 600-second upload
lifetime, and local compression evidence must not be substituted for that test.

Web `83aa8a36` adds explicitly labelled configured CPU/RAM capacity only when
observed limits are absent and the pinned public revision matches; unavailable
or invalid reads retain truthful absolute values. It also removes completed
preparation, preserves manual upward scrolling while following live growth by
default, restores square 34/44-pixel Send controls, and bounds long mobile
conversation content. Its 895 tests and four desktop/mobile theme browser
configurations passed; final production verification is still pending rollout.

One automation browser cookie appeared in a scratch transport diagnostic. Its
exact login record was revoked and replay with the original cookie returned
401. A fresh sign-in-only check in `login-alone-moMaAS/` created zero resources;
the NyxID CLI credential was unchanged. The earlier first/repeat evidence is
retained, but its revoked authentication state must not be reused.

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
