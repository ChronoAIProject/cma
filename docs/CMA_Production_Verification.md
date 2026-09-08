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
| 3. Public Sandbox Profile | 4000 millicores, 8192 MiB, two requested services, 4/4 limits, working toolchain | Exact live configuration and all four actual Agent workload checks passed on runtime `137b489d` at original heavy-workload base `edc52b0f`; independent receipt confirms unchanged tracked source. This does not establish workload behavior at newer `dev` revisions |
| 4. Public Trigger | Exact title/message, all-level reuse, Try It, visitor theme, no review, README placement | Exact revision published after first rollout; README-only PR #841 merged after all 12 protected checks; default README snippet read back exactly |
| 5. Login without bootstrap | No Sandbox, Workspace, Agent, or Profile created by login alone | Existing-owner fresh-browser sign-in created no resources; successful unseen-CMA-identity callback contract passes default/home/Trigger returns; external fresh signup not tested |
| 6. Complete external-click flow | Login then automatic provisioning; exact first message once; correct software, skills, scopes | `lifecycle-postfix` first journey passed at 02:09:30 UTC on API `6eca81a3`/web `4ddcd1b9`, with one first prompt after automatic recovery from an 18-minute startup-publication delay; not a smooth-start pass. Fix `a52e72ba` is tested and building, not deployed. Prior configuration/workload/restored-file proofs remain source-bound; final fresh configuration and genuinely paused README repeat remain held |
| 7. Previous UI style | Original rotating 3D mark and established controls; desktop/mobile screenshots | Recovered `lifecycle-postfix` title matches before reload and its settled mobile capture is readable; the earlier `lifecycle-fresh` resize artifact remains unaccepted. Prior motion/four-view proofs remain historical; final hierarchy/four-view layout and corrected fixed-step provisioning checks remain pending |
| 8. Autonomous YOLO | Pinned provider runs full-access mode with no approval requests | Actual provider configuration verified; latest first response and successful subsequent workload completed with zero approvals |
| 9. Lifecycle | Active to paused to active; paused to terminated; history retained | Manual Resume passed at 00:24 UTC. Fresh `lifecycle-postfix-delete` Pause and paused Delete passed on API `6eca81a3`: Paused revision 25, then Retired revision 36 at 02:20:02 UTC, with physical CR/pod absence, no writer debt, unchanged native identity/generation 1 and full history. The original older paused Delete timeout remains failed despite its separate recovery; final paused README revival remains pending |
| 10. Cleanup | Only identified stale state removed; new published flow remains usable | Identified old Sandboxes, heavy baseline, original lifecycle-fresh and disposable lifecycle-postfix baseline have retired through their admitted owner commands; physical absence is verified for the latest disposable baseline. No active mutating verifier remains as of 02:22 UTC; the final retained environment is not yet created or accepted |
| 11. Conversation UI | Thinking, Send/Stop, CPU, RAM, context, token, network metrics | Prior four-view metrics, Thinking, scrolling and lifecycle input holds passed; latest fresh-baseline UI test sent one message, observed its exact tool command, interrupted once, verified the public interrupted outcome, restored Send and preserved reloaded history |

As of 2026-09-08 02:22 UTC, API `6eca81a3` and web `4ddcd1b9` are live; runtime
`137b489d` is unchanged. The API deployed through the separately named exact
stopped-orphan recovery preflight, not an all-quiet claim. Original retirement
continuation passed at 01:37 UTC with the Agent stopped, exact history retained
and physical CR/pod absent; the timed-out original browser attempt remains failed.
A fresh normal all-owner guard then passed before the completed web rollout;
independent API/runtime/store/history recovery verification passed at 01:40 UTC.
Earlier `lifecycle-fresh` provisioning/first prompt, Send/Stop and manual Resume
passes remain tied to their original images. The single disposable
`lifecycle-postfix` journey completed after startup recovery, and its fresh
Pause/Delete sequence now has passing physical evidence. All three verifier
processes have exited. Startup fix `a52e72ba` is pushed and locally verified;
its API image build, started at 02:17 UTC, is pending. No deployment of that fix
is claimed. Final fresh configuration, Pause, README revival and hierarchy/UI
checks remain held until the verified image is deployed at a fresh quiet
boundary. No heavy workload replay is added. The historical deployed-component
table below is not the current campaign's image inventory.

The recovered first journey exposed a startup race: attaching the Trigger's first
prompt changed the Agent revision while AddWorktree ran, so its completed work
could not publish until the 18-minute lease expired. The same admitted operation
recovered automatically at 02:00:10; its native conversation opened at 02:00:18
and the first journey passed at 02:09:30 with its exact first prompt once.
No records were repaired or Launch repeated. The publication fix and regression
tests are complete locally, but this recovery is not a smooth-start acceptance
and the fix is not yet live. Actual `dev` base `33a85901` has unchanged
toolchain declarations but changed executable source, so earlier workload
evidence does not establish this new source's build or resource behavior.

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
the combined API `619535ad`, deployed below; no intermediate `d46abe08` rollout is
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

### Corrected API Deployment

API `619535ad7b898b79b7932f8f0c3802dce4a43ae4` combines the restore-bootstrap
correction with a fixed 1,200-second budget for newly staged checkpoint uploads.
Previously recorded deadlines are not extended. The 180-second per-request
timeout, four concurrent 32 MiB parts, three bounded part retries, integrity
checks and outer 1,800-second capture limits remain unchanged. Its image build
completed at 2026-09-07 23:31:36 UTC, the image scan passed, and registry push
with exact digest readback passed at 23:32:36. Manifest commit `077cefa4` pins
digest `sha256:a0c90ef63c8a6cb9edcf6eeb0b75dda267ac6036a1a9a1186b5a0e35f63e8674`
and was pushed to the campaign branch. These image checks do not establish
successful production upload, restoration or final visitor-flow acceptance.

The first all-owner guard was blocked at 23:33:23 UTC and remains preserved in
`all-owner-rollout-guard-pai6mF/`. It classified retained conversation metadata
on stopped Agents and an old terminal Agent-creation failure as runnable work.
The scratch verifier was corrected against the source-backed terminal-state
rules, with 20 passing positive/negative tests. Historical fields remain visible;
only exact stopped/retired, work-free cases are classified as retained metadata.
Pending work, leases, queued messages, writer debt and unsettled resources still
block rollout. No production record or runtime was repaired to make this pass.

Fresh `all-owner-rollout-guard-a4AxzC/result.json` passed at 23:41:02 UTC over
all eight Sandboxes, seven Workspaces, eleven Agents and twelve Launches across
the one owner present. The consistent snapshot found retired Sandboxes without
runtimes, capture, writer debt or lifecycle work, and no runnable resource work.
This is a point-in-time guard, not an atomic writer lease or independent
deployment authorization. Deployment ran in a separate completed step at
**23:41:16 UTC**, after the guard had returned, rather than sharing an
unconditional script with asynchronous polling.

`rollout-619-independent-LBWQn8/result.json` independently passed at
23:42:31 UTC. API pod `cma-api-645895fd68-v4g4m` was Ready on the exact pinned
digest above, its container image identity matched, and the owner session returned
200. Web `1eb60be1` and configured runtime `137b489d` were unchanged. This accepts
the guarded deployment only; it does not relabel the earlier 22:57 UTC rollout
error, expired upload or failed browser journey.

### Latest First Journey And Send/Stop

`production-lifecycle-fresh/result.json` passed at 2026-09-07 23:58:12 UTC on
the latest API, web and runtime above. Launch
`lch_18206b47cf8546f3ac0cf064591e4e5b`, admitted at 23:42:57 UTC, created Sandbox
`sbx_b8afb86d51604388ba653dd7d2d1fc5b`, Workspace
`wks_268b7288a84246e990dde584460a46cf`, Worktree
`wkt_0dcac4d41387460f9de1727a4c68dad3`, and Agent
`agt_8048f1e694c84c14837e4fd05f83a760`. One README anchor click and one
existing-owner OAuth exchange automatically reached the Agent, with no further
CMA click or approval. The exact published first prompt occurred once in one
completed turn; its eleven-message transcript SHA-256 is
`a6f0954fca752840e85b7290244e9a84605b02e8be78ef35a2b564d253a24af6`.
The response completed at 23:58:06, and all 494 recorded direct reads returned
200. Saved records independently agree on the four identities, all three
profile pins and the Agent's distinct Worktree.

The browser method remains GitHub-API-rendered README HTML at the source origin,
not authenticated GitHub web-page access or fresh signup. Local review of
`03-agent-desktop.png` found readable content but a stale Untitled agent heading
despite the saved generated title. `04-agent-mobile.png` was captured during the
immediate resize transition and clips most conversation/composer content; it is
not a settled mobile layout pass. `05-return-mobile.png` is settled and readable,
with the generated title. Original evidence remains unchanged. Future captures
wait for loaded fonts, completed finite transitions and three identical valid
geometry samples before accepting screenshots; eight offline tests cover this
guard. Explicit Home-to-Sandbox-to-Workspace-to-Agent selection on the final
environment is separately prepared, not yet production-run.

`send-stop-b0PDOD/result.json` passed at 23:59:13 UTC. Exactly one owner message
was submitted, its exact tool command was observed, and one real Stop produced
the public interrupted outcome. Send was restored and reload retained the full
history. The original first-turn hash above is unchanged; the resulting two-turn
hash is `a7f14bdfed13317a664854f8e0a4f7744eae79c7b350fcd4b7f13d1857b69783`.
No production response was replaced and no message was replayed.

### New Repository Base

The latest Worktree records `dev` commit
`f050cf57515d639767ff8a9e74ecbfeb9480a8a6`, 65 commits ahead of the heavy-workload
base `edc52b0f71fc5b3a55b446965ce50870e98898e7`. The read-only compatibility
report `trureturing-base-compatibility-f050cf57.md` used complete Git-tree
comparisons after the compare endpoint reached its 300-file cap. Toolchain pins,
Lake configuration/manifest, .NET dependency/build properties, tooling project
files, package locks and shell build scripts are unchanged. This is not a
documentation-only advance: workflow sequencing, Trureturing CLI Worktree and
Scribe behavior, ten added Lean modules and another compiled proof changed.

The comparison finds no manifest-level requirement to change CMA's software
profile or runtime. It does not prove the changed tooling commands, new source
builds or resource consumption. The actual heavyweight workload and independent
selected-artifact restore pass remain attached to the old base; they are not
transferred to the latest source. Final admission-bound configuration evidence
must record its actual new Worktree base independently.

### Manual Pause And Resume Preflight

`lifecycle-lifecycle-fresh-manual-pause/result.json` passed at
2026-09-08 00:11:05 UTC after one supported UI Pause. The exact Sandbox advanced
from Active revision 15 through Pausing revision 17 to Paused revision 24 at
00:11:03.993 UTC. All four identities and the complete two-turn history were
retained. This is the latest manual Pause pass, separate from historical heavy
Pause timeouts and their later observations.

The first Resume verifier failed before dispatch, with **zero commands** in
`lifecycle-lifecycle-fresh-manual-resume/failure.json`. Its preflight incorrectly
required a live `runtime_version` on the paused REST resource, which has no live
runtime binding. The failed evidence remains failed and preserved; no product
record or runtime was repaired. The scratch-only correction accepts an omitted
paused binding only alongside the exact completed Pause proof: same identities,
completed Paused revision, pre-Pause image, no pending lifecycle/checkpoint, and
the entire unchanged transcript. Eleven offline guard groups pass. The revived
live runtime image must still be checked after Resume.

The guarded manual Resume in
`lifecycle-lifecycle-fresh-manual-verified-resume/result.json` passed at
00:24:47.869 UTC. It used the exact preceding `lifecycle-fresh-manual` Pause
proof and one UI revive command, advancing Paused revision 24 through Reviving
revision 30 to Active revision 45 at 00:24:46.002. The four identities and
complete two-turn history were retained. `sandbox-after.json` confirms the live
runtime binding is again the exact `137b489d` image digest. This successful
supported Resume does not relabel the first verifier's zero-command failure.

### Paused Delete Blocker

The second UI Pause in `lifecycle-lifecycle-fresh-delete-pause/result.json`
passed at 00:27:33.552 UTC, retaining the exact hierarchy and complete two-turn
history. The Sandbox advanced from Active revision 45 through Pausing revision
47 to genuinely Paused revision 51 at 00:27:31.693. That exact completed Pause
supplied `CMA_PAUSE_PROOF_LABEL=lifecycle-fresh-delete` to the Delete verifier.

Exactly one UI Delete was admitted at 00:28:18 UTC, from Paused revision 51 to
Retiring revision 54. Verifier `21627` timed out around 00:58:18 UTC with
`Lifecycle did not settle`; its original
`lifecycle-lifecycle-fresh-delete-delete/failure.json` remains failed. The saved
command list contains only that DELETE and the browser-error list is empty.
There is no active verifier from that attempt and no repeat Delete is allowed.
The durable operation remains the recovery target, not a new admission.

At the blocked observations, Sandbox `sbx_b8afb86d51604388ba653dd7d2d1fc5b` was
Retiring at `stop_agents`; Agent `agt_8048f1e694c84c14837e4fd05f83a760` was
Paused at `close_process`, with
stop reason `sandbox_retired`, no active/queued turn, and original conversation
`01a07e44-0ac6-78b3-a0b4-b769a2e315c0`. The Sandbox runtime binding was absent,
but the runtime itself was not absent. Sandbox generation high-water was 3;
runtime-loss parking advanced the Agent conversation fence to 4.

The retained BatchSandbox handle `56eda9aa-b1b8-4692-b97d-4e2f5187365a` has UID
`5847cd9c-d114-4867-9093-3bdebd248e2c`, creation time 00:17:33 UTC, Kubernetes
metadata generation 1, phase `Succeed`, and policy `Retain`. Its replacement pod
`56eda9aa-b1b8-4692-b97d-4e2f5187365a-0`, UID
`068bdf4b-b74f-4bd2-aca8-dfa33562c65a`, was created at 00:28:43 UTC and remained
Running/Ready, with PodsReady observed at 00:29:53 UTC. Its labels bind it to
`chrono-bot-prod`, this exact Sandbox and runtime generation 3; Kubernetes
metadata generation and CMA runtime generation are different fields. Provider
GET and LIST both reported the same handle Running at 00:47:25 UTC. A successful
UI Pause result therefore does not prove physical provider teardown.

The exact read-only process observation in
`stopped-orphan-inspection-2KT6sP/result.json` passed at 00:56:18 UTC: zero Agent
homes and zero active provider processes. The observed processes were `dash`,
`layer4-runtime`, `execd`, and the inspection's `node` process; none carried an
Agent home. This proves the bounded stopped-orphan observation only, not rollout
authorization or cleanup. The earlier 00:53:17 process-inspection assertion
failure in `stopped-orphan-inspection-7gMCMe/` remains preserved.

### Paused Retirement Corrections At 01:00 UTC

The combined API source includes four committed corrections:

- `105dfc11` permits runtime-free Agent retirement only with exact parent,
  ownership, stopped-work and retained-checkpoint proof, transactional parent
  serialization, and fresh provider inventory proving no matching runtime.
  It preserves history and does not invent a runtime-close receipt or turn.
- `0dff0be8` retains Pause runtime authority and its failure when teardown fails;
  an empty provider inventory after an error is no longer accepted as success.
- `48c33777` retries Kubernetes cleanup conflicts at most three times while
  preserving exact UID, positive metadata generation, labels, owner references,
  specification, and UID/resourceVersion DELETE preconditions.
- `6eca81a3` accepts the actual Agent-4/Sandbox-3 parked fence only within a
  checked positive high-water-plus-one bound. Equal-generation orphan reclamation
  requires an unbound Paused/Retiring/Retired parent without a provision
  reservation, lifecycle operation or checkpoint capture; bound runtimes,
  initial publication and newer allocations remain protected.

Local integrated verification passed 581 core tests, 508 CMA tests with 169
ignored, 15 reconciliation cases, five lifecycle-failure cases, 119 adapter
tests, strict Clippy, formatting and diff checks. The exact 3/4 generation
shape and refusal/rollback cases passed against a fresh disposable local Mongo
replica set, which was removed. The broad CMA run used
`RUST_MIN_STACK=33554432` after an existing large `creation_revival` test overflowed
the default stack; the larger-stack pass does not erase that limitation.

API source `6eca81a35c06ee486aa1fed8cd15f3a3d557ca81` began image build `97050`
at 00:58:55 UTC; build, scan, push, digest pin and deployment remain pending at
this update. The ordinary guard in `all-owner-rollout-guard-TrKbTi/result.json`
correctly returned blocked at 00:50:05 UTC. A separately named stopped-orphan
recovery preflight is being prepared using the exact process proof above; it has
not run and no recovery rollout is accepted. It must not weaken the normal guard
or mislabel a recovery deployment as all-quiet. No record repair, manual pod
deletion, duplicate lifecycle command or new Launch is used.

At the 01:00 UTC update, guarded deployment and a separate read-only continuation
were still required to verify the original admitted retirement. Their subsequent
completion is recorded below. It establishes durable recovery, not a passing
original browser attempt or fresh post-fix Pause/Delete acceptance.

### Generated Title Correction

The stale first-run heading was reproduced locally: the sidebar's existing
hierarchy refresh saw the generated title after the first completed turn, while
Agent detail polling had already stopped and its header/tab remained Untitled.
Source `4ddcd1b9` passes the existing navigation metadata to the Agent controller
and updates only title metadata. Exact Agent, Sandbox, Workspace and Worktree
identities, a nonempty title and a nonolder observation are required. There is no
additional polling, conversation reload, new route or style change.

Eleven focused regressions pass, including the previously failing delayed-title
Console test. The same focused composer DOM node, unsent draft, rendered messages
and history are retained, with no extra Agent/transcript reads or message sends.
Unrelated, blank, malformed-time and older metadata are refused; previously
learned titles survive absent/older snapshots and do not leak across Agent
navigation. All 1,005 frontend tests in 89 files, typechecking, lint, formatting
and scoped secret scans passed. Source
`4ddcd1b9b10ce7b33132ee085d1de493d089559a` completed image build, scan and registry
push/readback at 00:36:56 UTC, with digest
`403552285bea68cb5fd1ea6c33a3ad99deaa5370dc697560044f39d1195d2b9a`.
It was not deployed at the 01:00 UTC update. The later live rollout is recorded
below; final browser title acceptance still requires its own passing evidence.

Future first/repeat verifiers now require API/header/tab/tree title equality
before any reload, then verify the collapsed-mobile header/tab before and after
reload. Title observation performs only exact-Agent GETs and never uses reload
to make the assertion pass. The final hierarchy verifier checks the same title
when selected, expanded and reloaded, and requires passing pre-reload title
evidence from both final journeys. Twenty offline title, evidence, request-policy,
ancestry and settled-geometry groups pass with syntax and secret scans. These
scratch changes leave all earlier evidence untouched and have not run against
production.

### Recovery Deployment And Original Retirement

API `6eca81a3` completed build `97050` at approximately 01:25 UTC after a
26-minute release compilation. The first vulnerability scan was interrupted at
01:28:40 because the local Docker Desktop credential helper did not return while
the scanner downloaded its public database. No report or scan pass was produced;
`api-6eca81a3-scan-interruption.json` preserves that outcome. A rerun using an
isolated empty Docker configuration passed at 01:29 UTC with no High/Critical
findings. Registry push and exact digest readback passed at 01:29:47.650 UTC:
`46af74403c38318184c8c28f2afcd818878582af7a0ac486b23021145fae187a`.
Manifest commit `80cc7f85` pins that image.

Two initial direct deployment attempts refused expired recovery proofs before
any production write; the rollout-before artifact was absent at both refusals.
They are not successful deployments. The separately reviewed scratch dispatcher
requires a previously completed independent passing preflight, then completes a
new synchronous read-only preflight with exit 0 before invoking deployment. It
requires more than five seconds of dispatch margin, while the existing deployment
consumer rechecks expiry and its three-second margin before every patch. It does
not weaken the normal guard or run deployment unconditionally after polling.

Fresh receipt `paused-retirement-recovery-rollout-0vf3hI/result.json` passed at
01:35:37.068 UTC with `deployBefore` 01:35:43.000. The API deployment request was
recorded at 01:35:38.321 and the rollout became Ready. This is a guarded recovery
deployment for the exact stopped orphan, not an all-owner quiet-state deployment.
Earlier failed process/preflight observations and stale-proof refusals remain
preserved. No manual database repair, provider deletion or duplicate owner command
was used to force recovery.

`paused-delete-continuation-QTp5I9/result.json` passed at 01:37:01.179 UTC. The
same originally admitted operation reached Retired revision 334 at 01:37:00.142,
with the exact Agent stopped and all four hierarchy IDs retained. The entire
two-turn transcript hash remains
`a7f14bdfed13317a664854f8e0a4f7744eae79c7b350fcd4b7f13d1857b69783`.
The exact provider CR and runtime pod were absent. Continuation issued zero new
commands and rechecked the original failure and command evidence unchanged.
This is successful recovery of the original retirement; the original thirty-minute
browser timeout is still failed and fresh post-fix Delete acceptance is pending.

The normal all-owner guard `all-owner-rollout-guard-z23CyR/result.json` passed at
01:37:33.423 UTC with no reasons. The web deployment request was recorded at
01:37:45.030; rollout session `67178` completed. Web `4ddcd1b9` is live at digest
`403552285bea68cb5fd1ea6c33a3ad99deaa5370dc697560044f39d1195d2b9a`, pinned by
manifest commit `4edcb796`. Both corrections are live; runtime `137b489d` remains
the same image.

Independent `rollout-6eca-independent-rpueuR/result.json` passed at
01:40:03.376 UTC: exact API digest and Ready pod, Retired revision 334, stopped
Agent with no active/queued turn or executable work, no writer/lifecycle/capture/
provision debt, retained checkpoint/native conversation/transcript, and original
CR/pod absent. The earlier observer `rollout-6eca-independent-EBnVqG/` used an
incorrect pod-label selector and was stopped before owner/store reads; its
failure and samples are preserved. Corrected inspection used the Deployment's
actual selector in a separate output directory. All independent production
inspection processes finished before the next single admission.

Root session `44753` is the sole active mutating verifier, following the already
started `lifecycle-postfix` first journey. Its saved `admission.json` records
Launch `lch_e3a9edc04f4c4414baf55156b3e582ff`, created/started at 01:40:47 UTC.
There is no passing result yet. Follow that existing Launch; do not rerun
admission, deploy again or issue another lifecycle command on resume.

### Fresh Physical Lifecycle Guard

Future Pause/Delete verifiers now require the exact deployed API image and capture
the store-bound runtime handle, generation, CR UID and pod UID before Pause.
Passing requires two consistent private-store observations without writer debt,
lifecycle operation, checkpoint capture, provision reservation or executable work,
plus complete CR/pod inventories proving no original or replacement runtime. The
exact hierarchy, native conversation and full transcript must remain intact.

Normal Pause preserves the Agent generation. Runtime-loss parking during Pause
may advance it exactly once only with a matching durable
`RuntimeGenerationRetired` event in the complete post-capture log range. Delete
and its immediate paused preflight must retain the completed Pause's actual
generation and accept no further retirement event. Resume reopens at the new
runtime's provision generation. Active's scheduled credential-rotation wait,
Paused's no-work state, and Retired's future purge wait are checked explicitly;
pending Agent stop requests or work are not accepted as settled.

The scratch guard passes 29 offline tests, syntax checks and scoped secret scans.
Delete refuses a legacy Pause without physical proof before dispatching anything.
This preparation changed no old evidence; the first fresh production results
are recorded below. The exact proof-binding procedure is also recorded in private
`lifecycle-physical-teardown-usage.md`.

### Recovered First Journey And Fresh Physical Lifecycle

`production-lifecycle-postfix/result.json` passed at
2026-09-08 02:09:30.324 UTC and verifier `44753` exited. The original Launch
`lch_e3a9edc04f4c4414baf55156b3e582ff` retained Sandbox
`sbx_24469556bd9744609cd07e7ef47f5841`, Workspace
`wks_7c0d57ba241849f8b689e5c786824bf5`, Worktree
`wkt_74fba9a17872411bbaba7dc66a67b22a`, and Agent
`agt_15c6521da781473fab05692cd22aa0dd`. One existing-owner OAuth exchange,
zero extra CMA clicks, zero approval commands and the exact first prompt once
are recorded. The title matched before reload; the settled 390 by 844 mobile
layout has no horizontal overflow or sidebar overlay, and local visual review
found `04-agent-mobile.png` readable. This does not erase the 18-minute startup
delay or establish final four-view hierarchy/provisioning acceptance.

Fresh UI Pause under label `lifecycle-postfix-delete` used one POST and advanced
Active revision 16 through Pausing revision 18 to Paused revision 25, observed
at 02:18:31.473 UTC. The browser result passed at 02:18:33.343; its separate
`physical-teardown.json` passed at 02:18:47.533 after two stable observations.
Verifier `25390` exited. The store snapshot
`store-lifecycle-1788833913194.json` records the published epoch-1 archive of
1,687,229,843 bytes, sealed at 02:14:45 UTC. This is the fresh pre-heavy-workload
home, not another heavy-workload replay.

Delete consumed that exact completed Pause proof, with the same label and
`CMA_PAUSE_PROOF_LABEL=lifecycle-postfix-delete`. Its saved commands contain one
DELETE, not a repeated request. `lifecycle-lifecycle-postfix-delete-delete/`
observes Paused revision 25 at 02:19:13.649, Retiring revision 28 at
02:19:30.669, and Retired revision 36 at 02:20:02.325. The browser result passed
at 02:20:02.753; physical postconditions passed at 02:20:17.036. Verifier `85477`
exited. The command list has no timestamp, so the first Retiring observation
is not represented as the exact Delete admission time.

Both physical proofs bind runtime handle `bd378519-abbb-4fc1-a5e3-a939f5dc760d`
and its original CR/pod UIDs, and prove no original or replacement runtime in
the complete inventories. Writer, capture, provision and lifecycle debt are
absent. Pause retained a Paused Agent; Delete retained it Stopped without pending
work or stop intent. The Sandbox's only remaining work is its future purge wait.
Native conversation `01a07ebe-a584-71c3-8b43-9e597333738c`, generation 1, all four
identities and the full transcript were preserved. History SHA-256 is unchanged:
`6edcfcdb228c43d2e9a2f2e74b3a3ff3465c0be7d1ac148ad313268baccbbc7a`.
The complete checked log ranges are 203 to 204 for Pause and 204 to 207 for
Delete, with no runtime-generation retirement event.

This accepts the fresh physical Pause and genuinely paused Delete on API
`6eca81a3`; it does not relabel the older
`lifecycle-lifecycle-fresh-delete-delete/failure.json` timeout or its separate
read-only recovery. No mutating production verifier remains as of this update.

### Startup Publication Fix At 02:22 UTC

Commit `a52e72ba76f9867cd5ce16e45022dbaafd201012` is pushed and independently
reviewed. It retries only definite startup publication conflicts, preserving
the actual completed effect evidence and native conversation identity without
rerunning runtime effects. Exact work/lease/evidence changes or observed parent
Pause, retirement or runtime replacement reject rebasing. First-prompt and title
metadata are retained, Ready and sign-in notices share the bounded retry, and
the publication clock includes claim, effect and retry elapsed time. Existing
strict store validation is unchanged; the retry parent read is a freshness
check, not atomic parent fencing.

The worker suite passed 49 tests with 15 integration/manual cases ignored,
including all nine new regressions. Strict all-target/all-feature Clippy,
formatting and scoped secret scanning passed. The final full CMA serial suite
passed 517 tests with zero failures and 169 ignored using the previously
documented `RUST_MIN_STACK=33554432` and `CARGO_INCREMENTAL=0`.
Keep the earlier test outcomes explicit: the default-stack run aborted in the
existing large creation-revival test; the documented-stack parallel run had
516 passes and one checkpoint tracing-log capture failure, with 169 ignored.
That exact tracing test passed in isolation before the full serial pass. Neither
failed run is rewritten as passing, and ignored cases are not executed evidence.
These test outputs are retained in tool-session history; no local test log files
were created.

The API image build for `a52e72ba` began at 02:17 UTC under root session `70420`;
`api-a52e72ba-build-request.json` records the exact immutable source and image
build arguments. Build, image scan, registry publication, manifest pin and
production deployment remain pending as of 02:22 UTC. Live API `6eca81a3`, web
`4ddcd1b9` and runtime `137b489d` are unchanged.

The executor's GitHub API readback confirmed repository visibility `internal`
and default branch `develop`; no visibility setting was changed. README browser
coverage remains GitHub-rendered HTML delivered at the declared source origin
with a real anchor click, not authenticated GitHub web-page access. Fresh signup
and cross-owner browser isolation remain untested with the single owner account.

### Pending Corrected Acceptance

The remaining superseding sequence in private `final-flow-audit.md` requires:

1. Finish API `a52e72ba` image build, scan and registry digest verification, then
   pin and deploy it only after a fresh completed all-owner quiet guard in a
   separate dispatch step. The disposable first/Pause/Delete sequence is complete;
   do not repeat its commands. Verify the deployed API before final admission.
   No heavy workload replay or extra manual Resume is added.
2. Run the final fresh README journey and admission-bound configuration observer
   as `final-fresh`, waiting for the observer's `owner_verified` marker before
   admission. Explicitly Pause that fresh post-fix baseline and independently
   verify physical provider-resource/pod teardown, not only the Paused API state.
   This retained environment is separate from disposable `lifecycle-postfix`.
   Then run `final-repeat` with
   `CMA_REQUIRE_PAUSED_REPEAT=1` and the exact completed Pause proof.
   The repeat must revive automatically, retain all identities and history, and
   add no first prompt, approval workaround or manual continuation click.
3. After final-repeat passes, run the prepared read-only explicit hierarchy and
   settled desktop/mobile light/dark checks without new messages or lifecycle
   commands. Twenty offline title, evidence, request-policy, ancestry and geometry
   tests pass; that preparation is not production acceptance.
4. Leave the final environment, published profile pins, Trigger and default
   README intact. Preserve all failed attempts and keep fresh-signup,
   cross-owner and authenticated GitHub-page browser limits explicit.

All future Resume, paused Delete and final-repeat verifiers must set
`CMA_PAUSE_PROOF_LABEL` to the exact preceding completed Pause label.
`lifecycle-postfix-delete` is the completed fresh Delete proof; use
`final-fresh-readme` for final-repeat. Labels are evidence bindings, not permission
to repeat a command.
The recovered original Delete used `lifecycle-fresh-delete`; the completed manual
Resume used `lifecycle-fresh-manual`. Leave `final-fresh` intact after final-repeat.

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
