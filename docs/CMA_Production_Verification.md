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
| 3. Public Sandbox Profile | 4000 millicores, 8192 MiB, two requested services, 4/4 limits, working toolchain | Exact live configuration and all four actual Agent workload checks passed on runtime `137b489d`; independent receipt confirms unchanged tracked source |
| 4. Public Trigger | Exact title/message, all-level reuse, Try It, visitor theme, no review, README placement | Exact revision published after first rollout; README-only PR #841 merged after all 12 protected checks; default README snippet read back exactly |
| 5. Login without bootstrap | No Sandbox, Workspace, Agent, or Profile created by login alone | Existing-owner fresh-browser sign-in created no resources; successful unseen-CMA-identity callback contract passes default/home/Trigger returns; external fresh signup not tested |
| 6. Complete external-click flow | Login then automatic provisioning; exact first message once; correct software, skills, scopes | Fresh `streaming-fixed` README entry, first response, live configuration and actual workload passed; heavy repeat restored the backend but failed automatic browser navigation; deployed web `1eb60be1` passes existing-ready-page recovery only; final fresh and genuinely paused README-repeat acceptance remain pending |
| 7. Previous UI style | Original rotating 3D mark and established controls; desktop/mobile screenshots | Live home/provisioning motion and four-view chat checks passed; web `1eb60be1` retains the tested lifecycle input holds and corrects automatic revival to the existing blue running state |
| 8. Autonomous YOLO | Pinned provider runs full-access mode with no approval requests | Actual provider configuration verified; latest first response and successful subsequent workload completed with zero approvals |
| 9. Lifecycle | Active to paused to active; paused to terminated; history retained | Heavy Pause settled at 22:12 UTC, backend revival around 22:36 UTC, and independent selected-artifact restoration passed at 22:52 UTC; original Pause timeout and repeat browser failure remain failed; later upload expiry and 22:57 UTC rollout during Pausing are not successful recovery; manual Resume and paused Delete remain pending |
| 10. Cleanup | Only identified stale state removed; new published flow remains usable | Identified old Sandboxes and the verified heavy test baseline were retired through supported owner commands; the latest Delete began from Pausing, not Paused; final environment remains pending |
| 11. Conversation UI | Thinking, Send/Stop, CPU, RAM, context, token, network metrics | Four-view active metrics, Thinking, live follow/manual scroll/re-follow, completion, hierarchy, reload and ready-page checkpoint input holds passed; actual Send/Stop acceptance remains pending on a fresh lifecycle test baseline |

The latest observed production versions are API `553e180b`, web `1eb60be1`, and
runtime `137b489d`. The heavy baseline's retirement was independently verified
at 2026-09-07 23:13 UTC. API `619535ad` and all remaining final-flow acceptance
are still pending; the historical deployed-component table below is not the
current campaign's image inventory.

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

### Streamed Checkpoint Rollout

At 2026-09-07 20:06 UTC, the corrected API `74e3a95b`, runtime `137b489d`
and web `8d517d62` were deployed from the campaign branch. API and web reached
Ready, and a public read confirmed the new lifecycle projection. Manifest commit
`14dd7398` pins the exact registry digests. All three images passed vulnerability
scanning; all 18 manifest resources validated without skips, with no policy or
HTTPRoute ownership errors. Immediately before rollout, the owner's seven
Sandboxes were retired and all Agents stopped or deleted.

The native-loaded runtime OCI manifest
`sha256:3c47370e24aa95915aa6c74567a981358ab333e44ba766ca4389e9827b32b5b8`
passed all 17 image contract checks and the non-root PATH/tool probe. Evidence
binds to that exact manifest and source revision, with no remaining disposable
containers or volumes. An earlier Docker-format import had identical executable
configuration and filesystem layers but a different manifest; its passing
evidence is retained separately and is not the deployed-manifest proof.

Web `8d517d62` passed 947 tests plus type, lint, format, CSS ownership and secret
checks. Local desktop/mobile light/dark browser tests verified editable drafts
and visible history during saving, failed-read input holds, automatic recovery,
admission-pending Starting without Stop, and Stop only for a real active turn.
Backend follow-up `2b930ca4` adds immediate checkpoint-input refusals without
queuing a later prompt, while preserving paused revival and durable Trigger
first-message retries. Its 569 core tests, 172 public tests, both SDK contract
projections and scoped lint passed. All 33 disposable real-Mongo contract tests
also passed, including the checkpoint-at-transaction refusal assertions; exact
container cleanup was verified. This follow-up API image is still building and
was not included in the 20:06 rollout.

Fresh campaign `streaming-fixed` admitted Launch
`lch_583a96ffe5aa4695a4ebe20f67e6397c` through one README click and one NyxID
OAuth exchange, without additional CMA clicks. It created Sandbox
`sbx_0354b0d3126a44ed84eba1d96f68bfff` on the corrected runtime. The actual
provisioning UI passed all four viewport/theme checks: blue running marks move,
green completed and yellow pending marks remain still, reduced motion is
honored, all canvas states match public Launch records, and layout does not
overlap. Evidence is in `live-provisioning-CRlmuc/`. Provisioning, first response,
heavy workload, lifecycle and final Send/Stop remain separate acceptance steps.

### Fresh Streamed Runtime Evidence

The `streaming-fixed` first journey passed at 2026-09-07 20:19:28 UTC:
one README anchor click, one existing-owner NyxID OAuth exchange, zero further
CMA clicks, the exact configured first prompt once, and a completed response.
Sandbox `sbx_0354b0d3126a44ed84eba1d96f68bfff` contains Workspace
`wks_5a7e012d331f482d9d063d1872b306d1`, Worktree
`wkt_b7ea8e2512394e69a3ed09f162f30d6f`, and Agent
`agt_7a91ca256fca4c0988b3dac711046e4d`. Its native conversation is
`01a07d7f-2374-7470-aace-8cd211f6ed26` on repository commit
`edc52b0f71fc5b3a55b446965ce50870e98898e7`. This remains a real browser click
on authenticated GitHub-API-rendered README HTML at the source origin, not
authenticated GitHub web-page access or fresh signup.

The initial configured-home checkpoint published epoch 1 at 20:24:43 UTC and
released its writer hold at 20:24:44. Scoped store readback confirms publication
and upload acceptance of the 2,125,877,335-byte archive, with the original native
conversation and no remaining capture. The workload draft remained unsubmitted
while saving and was sent exactly once after release.

The actual Agent workload completed at 20:32:39 UTC. Independent receipt
readback proves exit 0 for the .NET build (53.431 seconds), fast checks
(37.251 seconds), selftest (17.242 seconds), and Lean conventions target
(260.683 seconds). Tracked source remains unchanged, the receipt is a regular
mode-0600 file, and there were no approval requests. The original observing
harness failed at 20:28 and remains failed: its catch block discarded the
assertion details. Rate-limit warnings were observed at the same time, but its
original HTTP status was not recorded. Separate read-only recovery evidence
in `workload-recovery-inspect-streaming-fixed-j77bgY/` binds the exact original
admitted prompt, preserves the original failed evidence, and verifies completion
without replay. `workload-streaming-fixed/receipt-readback.json` independently
checks the receipt on the exact runtime pod.

`production-follow-0deyiO/` passed live SSE default follow, preserved manual
scroll position, Thinking disclosure, re-follow, completion, and durable history
in desktop/mobile light/dark views. A temporary sidebar reconnect warning was
visible during the concurrent observers; these results are not a claim that no
read request ever failed. `workload-streaming-fixed/heavy-home-before.json`
records the two-turn conversation, native identity, exact runtime digest, and
nine selected artifact hashes totaling 7,342,129 bytes. The matching replacement
pod and heavy-after evidence remain required; this is not a full-home hash.

API `2b930ca4` built, passed its image scan, and was pushed with exact digest
verification. The API-only rollout was requested at 20:54:14 UTC and was Ready
by the 20:56:01 readback. Its digest is
`sha256:d011a9d9de2fabbda7264d2f3918c27e5520470192be5df2ee75624c83cff1a2`.
Manifest commits `60cd2354` and `01b14052` pin the image and enable scoped
checkpoint diagnostics. All 18 manifests validate with no skips and no policy
errors. Web and runtime pins are unchanged. The same runtime pod UID and exact
two-turn transcript hash survived this control-plane rollout; no Agent turn
was active and no Sandbox pod was restarted. The admitted capture remained
recorded for lease-fenced recovery rather than being manually repaired.

### Heavy Publication Investigation

The post-workload capture began at 20:32:43 UTC and sealed a
6,265,977,876-byte runtime archive at 20:41:03.616. It did not publish a durable
checkpoint. The runtime released its hold at 20:46:27.501 and the controller
began another epoch-2 capture at 20:46:28; the last published checkpoint remained
epoch 1. The failed object's automatic cleanup began after its 20:51:03 deadline.
Metadata samples taken afterward returned 404 and cannot establish how much of
the archive had uploaded before failure. There was no API or runtime restart
during the original failure, and the live home and both completed turns remain.

Both four-view saving observers verified editable retained drafts, visible
history, disabled Send and no spurious Stop, but neither proves durable recovery.
The first timed observation is honestly partial; the second was stopped locally
after the failed publication was identified. No UI availability gap is accepted
as proof of a published checkpoint. Full built-home pause/restore remains blocked
on this failure, with the original evidence retained.

Commit `eac56ab1` adds a transaction-enforced 120-second quiet interval before
periodic checkpoint admission, preventing the observed immediate post-reply
input hold. It does not change explicit Pause, rotation, ongoing capture recovery,
or the heavy-upload path. All 1,052 affected core/CMA tests and 36 isolated real
Mongo contracts passed, along with strict lint, formatting and secret checks.
Its API image is building and is not yet deployed. A separate CMA-only immutable
part-recovery correction is being developed for transient transfer failures;
it is not yet production evidence or a confirmed explanation of the original
failure.

Commit `c60a4371` adds a successful previously unseen CMA identity callback
contract, using the real callback router and a loopback identity-provider mock.
Default, home and Trigger returns create only authentication, browser-login,
user-directory and authentication-audit state; callback composition supplies no
aggregate/profile write service. Session reads and callback replay do not add
duplicate user/audit records. All 98 focused auth tests passed, with four separate
Mongo tests ignored. This is not a fresh external NyxID signup or second-owner
browser test.

### Published Heavy Checkpoint And Mobile Navigation

The later automatic capture published epoch 2 at 2026-09-07 21:16:07 UTC.
Object `cpa_dc1adcafe24d41a99dbf861849fb97e3` is upload-confirmed and published,
with 6,265,720,509 bytes. Its staged transfer took approximately 443 seconds,
within the existing 600-second lifetime. This succeeded on API `2b930ca4`, before
the immutable-part recovery fix; the original failed transfer's exact cause
remains unknown. The original failed evidence is not relabeled. Read-only
publication evidence is in `heavy-publication-watch-FdXKFc/`.

The waiting UI verifier admitted one Pause at 21:16:10 UTC after authoritative
eligibility and unchanged hierarchy/history checks. Its new full-home archive
began uploading at 21:24:21. At the 21:24:26 store readback, the Sandbox was still
Pausing; this is not yet a completed Pause or restore proof. The original runtime
UID and two-turn transcript remain unchanged.

Commit `78a25c07` adds bounded recovery for immutable parts and the final manifest:
uncertain uploads are reconciled by exact-key full-size/SHA-256 readback before
reposting; confirmed uploads retry only their readback. The three-attempt bound,
server Retry-After, cancellation, private diagnostics, four-part window, 32 MiB
parts, all integrity checks and 600-second upload lifetime are tested. All 25
focused storage tests, 1,067 core/CMA tests and 36 isolated real-Mongo contracts
passed, along with strict lint, formatting and secret checks. This fix is not yet
deployed. The separate `eac56ab1` image finished building, but is held for a
combined API update rather than a redundant rollout.

The settled hierarchy initially passed four read-only views, but the subsequent
navigation interaction exposed a mobile drawer that obscured the selected Agent.
Web `242ae95c` closes only the narrow-screen expansion when an Agent is selected,
including the current Agent, and preserves desktop navigation and same-Agent
drafts. Six new viewport/navigation regressions reproduce the old failure; all
953 frontend tests and strict checks passed. The built image passed its scan and
remote digest readback. A web-only rollout was requested at 21:24:06 UTC and
completed on pod `cma-web-8677874dbc-dpq2z`, with digest
`sha256:f5b5b3d7a1677c7107fcea3957b18243a88bf37af0bd9c6200cd7c81b6595d1d`.
All 18 manifests validated without skips or policy errors. API and runtime pins
were unchanged. Post-rollout interaction evidence in `settled-sidebar-ViEEiz/`
passes automatic mobile closure, retained desktop navigation, exact rendered
history and all 168 observed GET responses; settled mobile screenshots are being
checked separately after the navigation transition. The subsequent settled
mobile evidence in `settled-sidebar-bzbCiC/` passed both themes with 84 further
HTTP-200 reads, automatic drawer closure and no overflow. Both observers retained
the exact 20 rendered messages and closed their browser processes.

The first Pause capture did not publish. The recorded lifecycle advanced to
retry attempt 1 at 21:25:42 UTC, with the same requested Pause, runtime identity
and conversation. Its second archive `cpa_a523fe0054fd44f0b293c35b27baeaf4`,
6,265,740,096 bytes, began uploading at 21:33:54. Publication and completed Pause
remain separate requirements; the already published epoch-2 checkpoint remains
available. No extra Pause command, message or database repair was submitted.

Commit `0327a382` adds a durable 120-second cooldown after confirmed periodic
writer release, including failed captures. The model and transactional stores
refuse early recapture and timestamp forgery; legacy records gain a timestamp
only after a real completion. Manual lifecycle and existing writer-debt recovery
remain ungated. All 1,519 workspace tests passed with 168 intentional skips, plus
37 isolated Mongo contracts and strict lint, format, doc and secret checks.
The combined API image, including `78a25c07` and `eac56ab1`, started building at
21:32 UTC and is not yet deployed. Preserved test-failure explanations document
the one-line debug fixture boxing and two public fixture-clock corrections;
assertions and default stack limits were not weakened.

A further screenshot review exposed missing composer holds during explicit
Sandbox lifecycle operations. Commit `a5d6fb4b` holds Send and approval decisions
with the existing visible notice while preserving drafts, history, real-turn
Stop, and eligible paused-message revival. All 981 frontend tests and strict
checks passed, including 28 new regressions; its web image is building. This
production composer acceptance remains pending its rollout.

Four obsolete local runtime OCI archives and their extracted copies were removed
to recover approximately 3 GiB. Their published registry images, image metadata,
test records and failed production evidence remain available. The current runtime
archive, active source directories, shared Rust target and builder caches were
not removed.

### Lifecycle Composer Production Verification

Web `a5d6fb4b` rolled out at 2026-09-07 21:47 UTC to
`cma-web-7dbbf89fc8-wppmf`, with registry digest
`sha256:61d0c52e45b508001e9381b8094a2d5e6b8534da0f13b81d62350beb63a09629`.
The scan, remote digest verification and all 18 manifest validations passed.
API and runtime pins were unchanged. Manifest commit `e94d5d46` and the source
commits were pushed to the campaign branch as `chronoai-shining`.

Production evidence in `pausing-input-DTMHTk/` passed desktop/mobile and both
themes on that exact web build. During the real Pause capture, a nonempty draft
remained editable, Send remained disabled, Enter submitted nothing, and the
accessible lifecycle notice remained visible without overlap. All 20 rendered
messages retained SHA-256
`e11477fb4dde0650b645b8479f01fbfd41baf1d0f20a60b721abeab6181f0604`.
All 168 observed requests were GETs returning 200, with no mutations, replaced
responses, browser errors or rate limits. Root inspected the mobile-light and
desktop-dark screenshots. The browser closed and its PID was confirmed absent.
The additional four-view canvas checks sampled the 44-pixel loading character
during Home navigation, not the large settled Home character.

The original `lifecycle-streaming-fixed-heavy-pause/` test exceeded its
30-minute post-dispatch deadline and remains failed. Its one admitted Pause is
observed separately; no duplicate command or manual database repair was issued.
At 21:52:33 UTC the third capture had entered upload as object
`cpa_c5504a2420f8445c81f5a677e8786382`, 6,265,725,637 bytes, with staged deadline
22:02:04.218 UTC. The previous epoch-2 object remains published. Runtime samples
showed 416 MB of local archive growth over 16 seconds before that upload; the
current transfer must not be described as stalled on that evidence. The native
conversation, all four resource identifiers and two-turn transcript SHA-256
`2d50df491bf47c2555f4e634ce61ac59f15f95581b0f5f3df382c8178f8f6edc`
remain unchanged. The combined API image is still building; no successful Pause,
restoration or final Trigger acceptance is inferred from these observations.

### Heavy Pause Completion And Recovery API Rollout

The third Pause archive published epoch 3 at 2026-09-07 21:59:21 UTC. Object
`cpa_c5504a2420f8445c81f5a677e8786382` is upload-confirmed and published, with
6,265,725,637 bytes; its staged transfer took approximately 437 seconds. Agent
parking completed at 22:05:40, followed by runtime teardown and final Paused
publication at 22:12:00. The original pod is gone, the lifecycle operation and
work lease are absent, and public Resume is available. All four identifiers,
the original native conversation and exact two-turn transcript are preserved.
Evidence is in `admitted-pause-continuation-D14NcJ/` and the matching scoped
store readback. The earlier continuation observer was cancelled to correct its
handling of omitted optional public fields; its samples and cancellation note
remain in `admitted-pause-continuation-fcxlvm/`. Neither continuation changes the
original 30-minute UI test's failed outcome. The admitted operation took
approximately 56 minutes including its failed upload attempts.

API `0327a382` passed its image scan, registry digest readback and all 18 manifest
validations. It was deployed only after the settled Pause, with no runtime or
Agent work in progress. Pod `cma-api-54d7684985-49mpn` became Ready on digest
`sha256:5278873cc69bd6ee43aef45c78cb8b725e312dbf1ec5214a0972fc089064f73a`.
Manifest commit `abd1821f` pins this image. Web `a5d6fb4b` and runtime `137b489d`
were unchanged. Readback is in `observe-api-0327-rollout-2214.json`.

One real repeat README click admitted Launch
`lch_5df2e48f722c4c2bb4d306cd50d7df28` at 22:13:55 UTC. It automatically selected
the existing Sandbox, Workspace and Agent, with no review or manual Resume, and
recognized the already completed first prompt. Replacement runtime
`f51eab7a-6281-4b9f-ab73-82580706f7fd-0` has UID
`c66ed753-b59f-41f2-9f58-17152c770999` and the unchanged pinned image. The
`production-streaming-fixed-restored-repeat/` run is still in progress; completed
activation and exact built-artifact restoration are not yet accepted.

Follow-up commit `553e180b` corrects backdated lifecycle failure events and
already-expired retry delays. Capture failures include monotonic elapsed work,
and writer recovery adds its own acknowledgement time before recording the
failure and scheduling retry. Four deterministic regressions failed before the
fix and passed afterward; 26 focused tests, 1,076 core/CMA tests, strict Clippy,
formatting and secret checks passed. The follow-up image is building separately;
it did not delay the verified `0327a382` rollout and is not yet deployed.

### Long Revival Browser Failure And Web Correction

The admitted heavy repeat reached backend Ready around 2026-09-07 22:36 UTC,
reusing the exact Sandbox, Workspace, Worktree and Agent. Both completed turns
retain SHA-256
`2d50df491bf47c2555f4e634ce61ac59f15f95581b0f5f3df382c8178f8f6edc`.
However, `production-streaming-fixed-restored-repeat/` failed at 22:37:53:
the browser stopped healthy progress polling after five minutes and never
navigated to the ready Agent. Its screenshot, failure and original admission
are preserved; no duplicate Launch or manual Check Again was issued.

Web `1eb60be1` changes the five-minute budget to uninterrupted read outages;
successful reads reset it, while terminal states and unmount still stop work.
Automatic revival uses the existing blue moving mark and In progress label;
genuine user-action blockers retain their presentation. Both thirty-minute
healthy preparation regressions reach automatic navigation. All 994 frontend
tests, typechecking, lint, formatting and secret checks pass. The scanned,
registry-verified image was pinned in `a733adaf`; all 18 manifests validated
without skips and policy lint passed. Web pod `cma-web-7944b84b65-wpwhg` is Ready
on digest
`sha256:9dd4ab08129f4a5d4a585976d3afff315a70ae2f878ebca24fd923f717700b12`.
Readback is `observe-web-1eb-rollout-2247.json`. API and runtime were unchanged.

The separate backend/data preflight passed and explicitly recorded periodic
epoch-4 capture. The first selected-artifact after-probe failed at runtime-files;
no artifact equality is accepted from that attempt. Its original wrapper
suppressed the subprocess detail, and the cause remains unclassified. A later
independent read-only probe passed as recorded below, without repairs or replaying
the workload. The original probe and browser failures remain failed.

API source `d46abe08` removes the unnecessary archive import before restore
bootstrap. Bootstrap only validates and records inherited installation identity;
actual credentialed home Restore retains transfer and integrity checks. Four
transport regressions and all 111 Layer 3 tests pass, with strict Clippy, format
and secret checks. Its duplicate-bootstrap-transfer correction is included in
the pending combined API `619535ad`; no intermediate `d46abe08` rollout is
required. The separate `553e180b` deployment and its timing error are recorded
below. Final acceptance still requires a genuinely paused README repeat, not an
already-active reopen.

### Ready-Page Recovery And Independent Heavy Restoration

Evidence in `ready-progress-1eb60be1-5Fje2G/` passed four fresh-browser views on
the exact deployed web build: 1440 by 900 and 390 by 844, light and dark. Each
opened existing Launch `lch_5df2e48f722c4c2bb4d306cd50d7df28` directly and
automatically navigated to the exact existing Agent. Both turns and all 20
messages retained the recorded transcript hash, and the rendered-message hash
matched across all views. The screenshots were visually reviewed without
overlap or viewport overflow. The real saving notice remained visible with Send
held and the draft editable. All 229 observed same-origin requests were GET/200,
with no mutations, replaced responses or browser errors. Browser cleanup was
confirmed. This is ready-page recovery and deployed-asset evidence only, not
production long-wait or paused-README-repeat acceptance.

At 2026-09-07 22:52:42 UTC,
`workload-streaming-fixed/heavy-home-after.json` independently passed against
replacement pod `f51eab7a-6281-4b9f-ab73-82580706f7fd-0`, whose UID differed
from the before proof. All nine selected artifacts, totaling 7,342,129 bytes,
matched their original hashes and modes, including .NET outputs, the Lean
build output and installed toolchain executables. The workload receipt, clean
tracked source, Worktree metadata, original native conversation metadata, four
resource identities and complete two-turn transcript matched the before proof.
Runtime `137b489d` was unchanged. This accepts independent selected-built-file
restoration, with runtime archive-integrity evidence; it is not an independent
hash of every restored home entry and does not convert the failed browser
journey into a pass.

### Upload Expiry And Nonquiet API Rollout

The next periodic epoch-4 archive, `cpa_bd562cae21cb4216b4b08d1b0a0a8440`,
contained 6,265,665,757 bytes but did not publish. Its recorded 600-second
staged-upload deadline expired at 2026-09-07 22:55:43.586 UTC. The prior epoch-3
checkpoint remained published and the runtime remained intact. The scoped
readbacks `store-lifecycle-1788821747087.json` and
`store-lifecycle-1788821878707.json` preserve the failed upload and subsequent
cleanup state; this is not successful periodic checkpoint acceptance.

Automatic Pause began at 22:55:57 UTC. The deployment executor incorrectly
initiated API `553e180b` rollout at **22:57 UTC**, relying on the earlier quiet
observation rather than a fresh completed pre-deployment check. This violated
the required quiet boundary. The abandoned Pause work lease still named the old
worker, with expiry 23:26:58 UTC; no successful recovery of that Pause is claimed.
`observe-api-553-rollout-2259.json` confirms the new API Ready while the Sandbox
was still Pausing. Pod `cma-api-685d9c4f9c-kkjmv` runs digest
`sha256:067e5f7e9ba039f749e29c1fbe60bb37b2510c1a26754fc56755f89dc9f88f81`.
Web `1eb60be1` and runtime `137b489d` were unchanged. Successful image readiness
does not excuse the deployment timing or establish interruption-safe recovery.

### Authorized Heavy Baseline Retirement

The verified heavy baseline was retired through one supported owner UI Delete
at 2026-09-07 23:06:09.619 UTC, after exact identity, completed heavy-after
proof, unchanged transcript, single Workspace/Worktree/Agent membership and
no active or queued user-turn checks. Its origin state was **Pausing**, not
Paused. `retire-verified-heavy-6PKnOO/result.json` observes Retired at
23:07:03.855 UTC, the Agent Stopped, and the original two-turn transcript with
SHA-256 `2d50df491bf47c2555f4e634ce61ac59f15f95581b0f5f3df382c8178f8f6edc`.
This is authorized test cleanup, not paused-Delete acceptance. No duplicate
Delete, additional workload or database repair was used.

Independent read-only evidence in `retirement-independent-NzwBth/result.json`
passed at 23:13:18 UTC: writer debt cleared, no lifecycle operation or runtime
version, Agent Stopped with reason `sandbox_retired`, exact two-turn transcript
retained through event sequence 255, and the exact replacement pod absent.
It used public GETs and Kubernetes inspection without runtime execution or
mutations. This completes the identified cleanup proof, not paused-Delete or
successful Pause-recovery acceptance. A fresh all-quiet observation is still
required immediately before the next API deployment.

### Pending Corrected Acceptance

API `619535ad7b898b79b7932f8f0c3802dce4a43ae4` combines the restore-bootstrap
correction with a fixed 1,200-second budget for newly staged checkpoint uploads.
Previously recorded deadlines are not extended. The 180-second per-request
timeout, four concurrent 32 MiB parts, three bounded part retries, integrity
checks and outer 1,800-second capture limits remain unchanged. Its image build,
scan, registry verification, manifest pin and production rollout are pending;
none of this source work counts as final production acceptance.

The 23:06 UTC superseding sequence in private `final-flow-audit.md` requires:

1. With independent retirement checks now passed, deploy verified API `619535ad`
   only after a fresh, separately completed all-quiet observation. Keep runtime
   `137b489d` and web `1eb60be1` unchanged.
2. Create a separate `lifecycle-fresh` README baseline and prove actual Send/Stop,
   UI Pause/Resume, and another UI Pause followed by Delete from genuinely Paused.
3. Run the final fresh README journey and admission-bound configuration observer
   as `final-fresh`, then explicitly Pause that baseline and run `final-repeat`.
   The repeat must revive automatically, retain all identities and history, and
   add no first prompt, approval workaround or manual continuation click.
4. Leave the final environment, published profile pins, Trigger and default
   README intact. Preserve all failed attempts and keep fresh-signup,
   cross-owner and authenticated GitHub-page browser limits explicit.

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
