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
| 3. Public Sandbox Profile | 4000 millicores, 8192 MiB, two requested services, 4/4 limits, working toolchain | Exact live configuration and four actual Agent workload checks passed on runtime `8ebb0db9`; independent receipt confirms unchanged tracked source |
| 4. Public Trigger | Exact title/message, all-level reuse, Try It, visitor theme, no review, README placement | Exact revision published after first rollout; README-only PR #841 merged after all 12 protected checks; default README snippet read back exactly |
| 5. Login without bootstrap | No Sandbox, Workspace, Agent, or Profile created by login alone | Existing-owner fresh-browser sign-in created no resources, including a new run on API `217dda6a`; fresh signup not tested |
| 6. Complete external-click flow | Login then automatic provisioning; exact first message once; correct software, skills, scopes | Initial corrected first click and new post-recovery README reuse passed; actual workload and configuration passed; fresh paused cycle and final clean first click pending |
| 7. Previous UI style | Original rotating 3D mark and established controls; desktop/mobile screenshots | Live home/provisioning motion passed; latest deployed chat passed four desktop/mobile theme configurations, including layout, hierarchy and reload |
| 8. Autonomous YOLO | Pinned provider runs full-access mode with no approval requests | Actual provider configuration verified; first response and subsequent workload attempt completed with zero approvals |
| 9. Lifecycle | Active to paused to active; paused to terminated; history retained | Legacy revival recovered automatically with exact history; normal checkpoint then one waiting command passed; full built-home manual lifecycle pending |
| 10. Cleanup | Only identified stale state removed; new published flow remains usable | Two old Sandboxes retired through the public API under explicit authorization; final environment pending |
| 11. Conversation UI | Thinking, Send/Stop, CPU, RAM, context, token, network metrics | Four-view active metrics, Thinking, live follow/manual scroll/re-follow, completion, hierarchy and reload passed; actual Send/Stop acceptance in progress |

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

### First Pause And Runtime Upgrade

The test Sandbox naturally paused at 2026-09-07 17:29:36 UTC with no outstanding
work lease, capture or lifecycle operation. Its two completed turns and original
conversation identity were unchanged. Epoch 5 retained a 1,680,789,718-byte gzip
archive; its verified upload took 123.315 seconds, within the 600-second upload
lifetime. Agent parking and teardown each replayed and verified that archive in
approximately one hundred seconds. This is the pre-build home, not acceptance
of the heavier repository workload's checkpoint.

API `f88d7cf0` and runtime `8ebb0db9` were then deliberately deployed at
17:30 UTC, after the old controller's long lifecycle lease was gone. Its
shutdown has no early lease-release path, so interrupting the prior capture
could otherwise have left work waiting for the 31-minute lease expiry. No
running Agent, writer token, or database record was manually repaired.
Web `83aa8a36` had deployed independently; web `e4f495fc` followed at 17:52 UTC.

Launch `lch_8591474c829b4d0191e8e6e2734eb7b7` came from a real repeat README
click at 17:31 UTC. It reused the exact existing Sandbox, Workspace, Worktree
and Agent, without a review step or another first prompt. The replacement pod
`dcaae4ca-8e74-4d51-81a9-bb378a897b32-0` ran runtime `8ebb0db9`. Bootstrap
import completed, followed by a second whole-home import, validation,
extraction and installation. Temporary import, staging and backup artifacts
were gone by 17:38:36 UTC. The original conversation
`01a07cc9-7c4e-7590-90f4-49c6da3fb499` reopened in runtime generation 7;
both complete turns remained byte-for-byte unchanged.

Final Active publication did not complete. A reconciler admitted
`PublishConnection` after lifecycle reopen; that worker required Active while
Active publication required the connection work to finish. Source `22e2bc98`
permits only validated restore-participant reconnection in the final two
restore phases, retaining quiet-work and writer-fence checks. It passed the
persisted-stuck and interleaving regressions, 35 Agent worker tests and 64
Sandbox worker tests. Further live inspection exposed a missing transport case:
lifecycle reopen used a resume-bearing request, whereas reconnection replayed
the original non-resume request. The live runtime journal and durable original
receipt had different request hashes. Source `217dda6a` adds a bounded,
exact-identity compatibility correction and canonical future restore request.
Only an exact structured, nonretryable identity conflict permits one retry with
the recorded conversation and unchanged settings; ambiguous failures and other
refusals never permit it. The original durable receipt remains unchanged.

The combined correction passed 103 adapter tests, 35 Agent worker tests,
64 Sandbox worker tests, a real supervisor journal/process regression, and
all-target/all-feature Clippy. Legacy and canonical restore tests cover three
adapter restarts and checkpoint generations, with negative cases for changed
settings, wrong conversations, corrupt receipts, and ambiguous responses.
The API image built successfully and passed the image scan before rollout.
The final pre-rollout sample at 18:46:46 UTC still had both completed turns and their
original conversation, with no queued message or manually repaired state.

The browser attempt `production-paused-revive-8ebb0db9/` timed out at its
thirty-minute provisioning deadline. It remains a failed attempt even if its
durable Launch later recovers. Evidence of successful file/conversation
restoration is not evidence of successful final activation. Read-only samples
and timings are retained in `rotation-observer-uAqj0t/`.

### Conversation Read Recovery

Real reloads intermittently returned authentication-authority 503 responses,
including event and sidebar reads, while other authorized conversation reads
succeeded. The previous client discarded already fetched history when event
pagination failed. Web `e4f495fc` preserves that history and draft, explicitly
marks incomplete activity, withholds dispatch/replay until complete history
is available, and retries from cursor zero with bounded backoff. Authentication
denials and malformed data remain failures. All 903 tests and four browser
configurations passed before deployment. A separate real hierarchy probe
confirmed the current Agent is present and navigable beneath its exact
Workspace; the similarly named top sidebar branch was another Sandbox.

The original upstream 503 cause is unproven. An eight-request cookie burst and
a separately authorized, session-bound probe of sixteen userinfo/introspection
pairs returned only 200 responses with unchanged durable authority. No 429 was
reproduced, and no credential, refresh, revocation or database mutation was part
of that probe. Source `3e1a755a` coalesces only unfinished verification for exact
authority identity, gives each caller its own deadline and durable recheck,
and adds static failure-stage/status diagnostics without a TTL authorization
cache. Its twenty cookie-authority tests, 97 broader auth tests and Clippy
passed. All four previously ignored Mongo auth integration tests subsequently
passed on combined source `217dda6a` against a fresh, loopback-only MongoDB
8.2.4 replica set. The tests removed their databases, and that exact local
fixture was shut down and its disposable data removed; no integrated or
production Mongo instance was used. The combined API source `217dda6a` was
subsequently deployed; the original upstream 503 cause remains unproven.

### Recovery Acceptance

API `217dda6a` became Ready on 2026-09-07 at 18:48 UTC, using digest
`sha256:e187c78f7398a5d43b2e217f22c5db8bc4bbf92dd6601851d7d8ddac21703226`.
Runtime `8ebb0db9` and web `e4f495fc` were unchanged. The existing Sandbox pod
was not restarted. The original pending Launch reached Ready at 18:48:52 UTC;
its connection work and lifecycle operation settled automatically. Two separate
readbacks verified the exact original conversation and both completed turns,
whose hash remained
`2ecee216215088857561e8eebd077acd9694e24f0e66d09c080b4cadc4dffebb`.

A new README click, Launch `lch_5dbef7fee22e402096d335db3d7d3af8`, passed
in `production-recovered-reuse-217dda6a/`. It reused every resource, preserved
the whole transcript and exact first prompt once, and required zero extra CMA
clicks or approvals. This still uses the documented rendered-README method;
it is not authenticated GitHub-page browser coverage or a fresh paused cycle.
The earlier thirty-minute timed-out attempt remains failed evidence.

The single real workload submission waited for scheduled checkpoint epoch 6.
AG-UI waits in memory for this boundary before durable admission, with a
ten-minute bound; an optimistic Stop button alone is not evidence of admission.
The 1,680,505,384-byte archive published at 18:53:21 UTC. Exactly one owner
message and turn then progressed through dispatch fencing, confirmed write and
observed execution, without a resend, approval or writer-hold deadlock.

The completed `workload-toolchain-fixed/` run passed all four actual checks:
`make -C tools dotnet` (151.209 seconds), `check-fast` (38.313 seconds),
`selftest` (19.166 seconds), and the Lean `D5.S0.Conventions.WDigits` target
(334.694 seconds). Independent receipt readback required all four real exit
codes to be zero, no termination signals, exact commands, the original worktree
and commit, and no tracked source changes. The earlier failed PATH workload
and its receipt remain preserved separately.

Live configuration verification passed while that workload was active. The
first restored-runtime probe failed because it expected a fresh completed
installer journal. Replacement runtimes correctly record inherited installation;
the corrected probe requires fresh exact-pod evidence tying both bootstrap
hashes to durable checkpoint epoch 5 and the original five-command installation
proof. Seven local positive/negative cases cover this branch. No installer,
runtime journal or database record was edited to satisfy it. Model `gpt-5.6-sol`,
`xhigh`, `never`/`danger-full-access`, instructions, four skills/20 files, native
discovery, software versions, login-shell PATH, worktree, environment and exact
NyxID services/scopes all passed. The built home had 346,931 entries and
13,472,969,062 payload bytes; its complete heavy lifecycle remains pending.

Fresh existing-owner sign-in on the new API passed in `login-alone-9L1NBT/`
with zero Trigger visits or resource commands and identical inventories before,
after and ten seconds after login. Existing sessions were preserved.
The latest UI evidence in `agent-ui-gLs7J7/`, `paused-hierarchy-cjTeuk/`, and
`production-follow-AoGPFn/` passed desktop/mobile and both themes: actual resource
metrics and pinned capacity, Context matched to public usage events, Thinking,
live follow, manual-scroll preservation, re-follow, completed response, exact
hierarchy, details and reload. Earlier incomplete verifier runs remain preserved;
their findings were not used for speculative product changes. Actual Send/Stop,
full built-home pause/restore, paused deletion and the final fresh environment
are still pending acceptance.

### Heavy Checkpoint Request Cancellation

The first full built-home checkpoint did not publish. Repeated epoch-7 captures
left the latest durable checkpoint at epoch 6, while the four completed turns,
resource identities and selected built artifacts remained unchanged. Runtime
inspection found a locally sealed 6,263,745,872-byte archive and overlapping
temporary capture files; local archive creation is not durable checkpoint
acceptance. The matching `heavy-home-after.json` restore proof has not passed.

Read-only inspection confirmed the consumed Sandbox provider uses an HTTP
client with a 180-second timeout. Its scoped control failures occurred at
19:12:38, 19:16:08, 19:19:38 and 19:23:08 UTC, each approximately 180 seconds
after endpoint resolution. Cancelling CMA's HTTP handler dropped the operation
and writer guards while its blocking archive task continued. No provider
configuration, runtime journal, writer token or database record was repaired.

The CMA correction keeps long control operations owned independently of the
HTTP connection and streams JSON whitespace until the exact typed response is
available. Archive import validation additionally negotiates
`application/vnd.cma.checkpoint-receipt+json`: opted-in clients require the
committed object reference, SHA-256 and byte count before accepting HTTP 200;
legacy clients retain the empty HTTP 204 response. Heartbeats are not completion.
The adapter's 107 tests pass, including delayed streamed completion, both failure
classes, legacy compatibility, altered identities, malformed/truncated responses,
and receipt size limits. All 168 runtime tests pass, with one existing external
fixture ignored. Three real HTTP-disconnect regressions prove held-capture
fencing and exact replay, ordinary-capture serialization, and inherited restore
completion. All-target/all-feature Clippy and secret scans passed for both layers.

The test-only Sandbox was retired through one actual UI Delete command after
two exact-identity, no-active-work and four-turn-hash checks. Its pod is gone,
writer capture and lifecycle operations are cleared, and all four public
conversation turns retain their original SHA-256. Its generated runtime home
was removed; `retire-checkpoint-failure-GDUebo/` preserves the evidence. This
does not count as a passing heavy restore or paused-deletion acceptance. Fresh
production verification on the corrected images remains required.

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
