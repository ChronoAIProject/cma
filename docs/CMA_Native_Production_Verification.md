# CMA native Sandbox production verification

## Local production candidates — PR #1027

Release preparation at **12:41 SGT** pins the locally built API source
`cef50ef8f821307e0981c122f17eff4116dc1201`, image
`sha256:779341fd1d4c458badc746a194268b09a66c6291d0900e9768d04efef54689f8`.
This candidate overlaps independent same-owner metadata reads after credential
resolution and adds safe connection-phase timings. All 36 relevant broker/browser
tests, strict Clippy and the local Linux/amd64 release build passed. Image
publication, scanning, rollout and final-candidate measurements are tracked
separately; this preparation entry does not claim their completion.

The completed Web53/API6571 series has four functional passes. Source-flow
Send eligibility was **3.530, 4.083, 4.124 and 3.991 seconds**; operation-success
observations were **4.723, 4.816, 4.870 and 4.733 seconds**. Durable input
acknowledgement was **4.277, 5.817, 5.620 and 5.484 seconds**. Each repeated
sample encountered one definite non-admission conflict, retried the identical
key and body, and completed exactly one logical input. Thus source eligibility
passed 4/4, durable acknowledgement passed 1/4, and both thresholds together
passed 1/4. Actual browser Composer timing remains unmeasured; these results
cannot serve as a browser upper bound. Evidence is frozen in
`/tmp/cma-source-web53-api6571-series-summary-20261015.json`.

Current follow-up at **12:26 SGT**: API `6571e110` is deployed and the connection
publication repair passed a fresh functional creation sample. Web `53bbe2f7`
completed rollout at 12:25:35 SGT, generation 110, image
`sha256:ce38cb25177810a323cf81adbba7674ca205b449e12eb5154b0202e2be002df7`.
The public and running-Pod asset `index-C3GzMrh2.js` both hash to
`6ea9604030383aa3d98082db420b837364bbccfa2ceb4813200bf007f0aa5bca`.
This Web change counts successful request time toward lifecycle polling cadence;
67 focused creation tests, type checking, lint and the production build passed.

The first Web47/API6571 source-flow sample executed its original first prompt
and queued user input successfully. Send eligibility took **5,223.763 ms**, and
the durable acknowledgement took **6,317.150 ms**, so its five-second timing
checks failed. A fresh Web53 sample is in progress; actual browser click-to-input
acceptance remains pending computer-use permissions.

Live Stop on API6571 passed: a fresh observed intent was accepted in
**305.879 ms**, the exact active response became interrupted about **4.039 s**
after intent, and one follow-up input was accepted in **777.614 ms** and completed.
The first attempt had returned a definite revision refusal and was not replayed.
The passing attempt encountered a transient Mongo write conflict and recovered
within the bounded retry budget. API identity remained stable with zero restarts.
Resume acceptance took **505.250 ms**, and actual runtime readiness took
**26,353.341 ms**. Original Claim, Sandbox, PVC, PV and conversation identities
were preserved. This run does not repeat the earlier full-home byte/metadata test.

Evidence: `/tmp/cma-web-rollout-53bbe2f7/verified.json`,
`/tmp/cma-source-final-web47-api6571-pilot01-evidence/report.json`, and
`/tmp/cma-stop-response-6571e110-complete-frozen-20261015-01/acceptance-report.json`.
The dated 12:16 checkpoint below records the preceding Web image and results
that were still pending at that time.

This is an interim observation at 12:16 SGT on 2026-10-15, not a declaration
that the complete acceptance bar passed. Authorized local builds are being
tested in production; source CI is recorded separately below. The PR remains
unmerged. CMA stays in its original cluster, runtime workloads stay in the
separate agent-sandbox cluster, and Image Manager remains undeployed.

| Deployed component | Source | Immutable registry digest |
| --- | --- | --- |
| Agent Manager, generation 31 | `6571e110d443bbb321251313f101702b05942a68` | `sha256:99044a2876b0919c680118d55e57883571b208bcffacd69aad689d6a932ea98d` |
| Shared Web, generation 109 | `47b56be7c35c89c3813f8da981af533f00b608d5` | `sha256:27a307765d40554c9a2afa399626abb9a0a5fcfcc4fd69461bd089c310ffbb85` |

Registry manifests, Linux/amd64 platform manifests and source labels were
checked against immutable local build metadata. These checks do not constitute
an independent signed source attestation. Fixed HIGH/CRITICAL vulnerability
scans passed. Web asset `index-Bt0dT2fp.js` has the same SHA-256 in the running
Pod and public response:
`2a9e09486bc9b5af4e89a1d73e934ea795abcaf89b57b5c043066710e748dda2`.
The preceding runtime `cad2058` and Event Gateway `0c306813` are retained.

Google authentication was restored, and the combined API image was published
and deployed. At **12:16:26 SGT**, generation 31 was Ready with the exact image
above and Pod UID `f6ed0152-f991-4b0a-8126-525a16121685`. The authenticated owner
session and both retained fixtures passed continuity checks: the protected
baseline kept all 15 turns, and the recovered fixture kept its completed turn,
identities and settings. Branch head `0c16330e` passed all 15 CI jobs; the
deployed image source remains `6571e110`, not that later branch head. The local
Linux/amd64 build and fixed HIGH/CRITICAL scan passed. Current browser/model,
fresh warm creation and live Stop acceptance remain pending.

The synchronized Deployment preserves runtime `cad2058`, the prepared target
of one, and singleton/Recreate behavior; the shared ConfigMap default remains
zero. Repository manifest checks passed: kubeconform v0.8.0 validated all 48
resources with strict schemas and none skipped, kube-linter 0.8.3 found no
errors, and HTTPRoute backend ownership matched exactly.

| Boundary and evidence | Result | Interpretation |
| --- | --- | --- |
| API9d warm pilot02: input eligibility / durable input / live connection / Agent Ready | 2,507.511 / 3,609.991 / 4,488.087 / 4,571.302 ms | One functional and API performance pass; not browser click/render proof |
| Web33f/API9d source-ordered sample01: Send eligibility / durable acknowledgement | 7,548.177 / 9,141.613 ms | Functional pass, both five-second boundaries failed |
| Web5c6/API748 source sample01: Send eligibility / durable acknowledgement | 3,584.134 / 4,276.849 ms | Timing below five seconds, but functional failure: first message remained queued and the input timed out |
| API9d pause acceptance / early resume input admission | 278.837 / 1,112.389 ms | Public acceptance passed; input was durable while Reviving |
| API9d actual resume / completed marker response | 25,048.418 / 31,099.333 ms | Underlying recovery remains slower than five seconds |
| API748 disposable source fixture deletion / native finality | 205.279 / 65,836.100 ms | Public deletion accepted quickly; native cleanup completed in background |

The source measurement runs actual production SDK, authentication, loader,
connection-preparation and input-admission functions. It omits React effects,
DOM rendering, browser connection reuse and background navigation traffic;
it is not a conservative upper bound for browser performance. This candidate's
browser timing, popup and external OAuth callback remain unverified at this
observation. The first connection
check was successfully reused once in the new sample; most remaining delay
occurred before the operation exposed its Agent, then during sequential loading.
The next Web5c6 sample improved input timing but exposed an execution failure:
Mongo write conflicts exhausted connection-publication retries after the new
18-minute worker lease was claimed. First-message delivery remained queued;
the ordinary command expired. Both acceptance flags remain failed. The repair
for known-aborted publication and exact-lease retry passed 124 worker/store
regressions and strict Clippy, including ordinary worker reconnection and
unknown-commit protection; the repair is now deployed and live verification
remains pending. The new Web47b source only extracts TypeScript declarations to meet the source
size gate; its emitted controller behavior is unchanged.

The latest resume retained the original Claim, Sandbox and PVC, replaced only
the Pod, and preserved verification fixtures throughout `/home` and
`/home/.codex`: exact bytes, metadata and links. Conversation identity and
history survived. The full-home mount uses
the original PVC's `home` subdirectory mounted at `/home`. A read-only native
timeline observed about 12 seconds from scheduling until Pod infrastructure
was ready to start containers, followed by actual runtime/protocol startup.
CMA did not modify the agent-sandbox controller or infrastructure.

Explicit Luna and gpt-6 turns completed in the same conversation on API9d;
runtime JSONL metadata independently confirmed both selections. Historical
Luna output retained its original model after settings were restored; old
turns without model evidence remained `unknown`. A previous service-scope
request settled with exactly one automatic notification and acknowledgement
on the preceding API `cd7e5d89`; that result does not prove the newer
contention repair or a fresh external OAuth journey.

The latest live Stop verification remains unsuccessful: observed cancel requests returned
HTTP 409, and the original retained key was ultimately proven not admitted.
The turn completed naturally. A production-shaped real Mongo test covers
active-target admission and same-key recovery, and deployed safe stage diagnostics showed that both inspection and admission
encountered transient Mongo transaction conflicts. Admission exhausted all five
attempts; no domain revision refusal was observed. The exact original key was
then definitively resolved as not admitted after natural completion. A bounded
backoff fix passed 17 real Mongo and 26 core regressions and strict Clippy, and is
included in the deployed candidate. No successful live Stop is claimed here.
An ordinary second Agent in a prepared Sandbox exposed an MCP authority
binding defect. The fix and real Mongo regression passed and the fix is deployed
in API748. A subsequent public functional diagnostic created one optional-GitHub
Agent in an existing paused Sandbox: request acceptance took 366.602 ms, the
original Sandbox revived automatically, and the exact repository first prompt
completed. The new Workspace source was `empty`, defaults and mandatory scopes
matched, and the original Agent's history was unchanged. Completion took
63,999.656 ms including revival and repository setup; this is not a warm-pool
performance result. At 12:16 SGT, a read-only native follow-up confirmed the
original Claim, Sandbox, PVC and PV identities, Bound home volume and whole
`/home` template mount. The Sandbox was Suspended with no owned Pods. This
closes native identity verification for the API748 diagnostic; browser and
final API6571 contextual verification remain pending. Exhausted
owner Sandbox quota is now a terminal creation conflict in production, rather than an
endless transient retry. Future samples must have quota headroom.

Two attributable old test Sandboxes were retired through one public DELETE
each. Requests were accepted in 221.418 and 222.041 ms; public Retired was
observed after 35.007 and 64.618 seconds, with both original transcripts
unchanged. The complete owner inventory then showed seven nonretired Sandboxes
and three quota slots available. At 12:16 SGT, complete native namespace lists
confirmed both original Claims, Sandboxes, PVCs and owned Pods absent, checking
exact names and UIDs, including terminating objects. This is cleanup evidence,
not a final-candidate deletion or browser performance pass.

Private evidence: `/tmp/cma-api-rollout-74842c4a`,
`/tmp/cma-web-rollout-47b56be7`,
`/tmp/cma-source-flow-5c6baab1-api74842c4a-sample-01`,
`/tmp/cma-stop-response-diagnostic-nextapi-20261015-01`,
`/tmp/cma-public-prepared-9d5f3dcc-pilot-02`,
`/tmp/cma-source-flow-33f8a5e0-api9d5f3dcc-sample-01`,
`/tmp/cma-resume-early-input-9d5f3dcc-complete-frozen-20261015-01`,
`/tmp/cma-model-retest-9d5f3dcc-complete-frozen-20261015-01`, and
`/tmp/cma-public-delete-74842c4a-source33f-01`. New build and diagnostic evidence
is in `/tmp/cma-api-local-scan-6571e110-final`,
`/tmp/cma-prepared-connection-recovery-validation-20261015.json`, and
`/tmp/cma-public-existing-sandbox-api748-diagnostic-20261015-01`.
The current rollout and continuity receipts are
`/tmp/cma-api-rollout-6571e110/provenance.json`, `verified-binding.json`, and
`continuity-recheck-01/verified.json` in that directory. Cleanup and native
follow-up receipts are
`/tmp/cma-old-sandbox-public-cleanup-20261015-01/report-final.json`,
`/tmp/cma-native-public-diagnostics-postauth-20261015-01/cleanup-native-absence.json`,
and `contextual-parent-native-identity.json` in the latter directory.
Manifest validation is recorded in
`/tmp/cma-api-manifest-validation-6571e110-20261015.json`.
These operator artifacts remain
outside the repository. Existing failed reports are preserved unchanged.

Every API reader and rollback image must include `c8ee3872` or an equivalent
reader for the durable model-bearing `TurnStarted` events already written.
Compatible fallback digest
`sha256:f5fdac8f8967f2f859cb4b65d25f0cb6b335b34baf6edd626025f6a7e85ef5fc`
is retained and scanned; it has not been rolled out for a live recovery test.
Never delete or rewrite history to permit an older strict reader.

## Full-home preservation and lifecycle performance — PR #1019

PR #1019 merged candidate `3d6885c6bba35f205e4b498800a1bb2128753ed3`
into `develop` as `68dbe4b653f80ed260ef9ddaa5c42ed301272d94`.
CI [38557313596](https://github.com/ChronoAIProject/cma/actions/runs/38557313596),
attempt 1, passed all 15 jobs. Agent Manager generation 17 and shared Web
generation 93 were Ready at approximately 20:26 SGT. Their published digests
are `sha256:ff464a85415309745791e8e7b967839613929bd73842e19ba5eb265534267705`
and `sha256:dc5b98bfebc3605597b2dd096f4bd807df459c667d8b62d4b5b8788303fcc951`.
Runtime `cad2058` and Event Gateway `0c30681` were retained; Image Manager
remained undeployed. The new exact default pool reached two observed Ready
replicas with `fsGroupChangePolicy: OnRootMismatch`.

One fresh production fixture produced the following browser observations.
Accepted feedback and actual completion are separate boundaries; a functional
pass does not imply the five-second performance target passed.

| Operation | User feedback / acceptance | Actual observed completion | Five-second result |
| --- | --- | --- | --- |
| Warm New Agent | Automatic exact Agent navigation and enabled composer: 17,305.6 ms | Same 17,305.6 ms boundary | Failed |
| Pause | Feedback 1.8 ms; accepted 183 ms | Agent paused 1,682 ms; Sandbox settled 8,961.1 ms | Feedback and Agent passed; complete Sandbox failed |
| Resume | Feedback 1,318.3 ms; accepted 1,570 ms | Agent ready 21,648.7 ms; Sandbox settled 22,876.3 ms | Feedback passed; usable Agent and complete Sandbox failed |
| Delete | Feedback 9 ms; accepted 355.8 ms; hidden from active tree in the same observation as acceptance | Background retirement confirmed 65,415 ms | Immediate disappearance passed; cleanup remains background work |

Pause/resume retained the original Claim, Sandbox and PVC identities. No owned
Pod remained while paused; resume created a different Pod using the same PVC.
All 15 inspected entries across `/home/cma`, `/home/.codex` and a separate
`/home` fixture matched exactly after resume, including bytes, modes, ownership,
symlinks, and staged, dirty and untracked Git content. The original conversation
identity and complete history prefix were preserved and a post-resume turn
completed. The recall prompt was ambiguous among four earlier markers: the
model returned the most recent original marker, while the helper expected the
first. That stricter assertion remains recorded as failed; no prompt was resent.

Real model selection/inference, Ornn skill installation/use, and the scope MCP
request → owner approval → delivered credential → exactly one automatically
acknowledged notification in the original conversation passed. The service was
already connected, so this does not establish a fresh live OAuth callback.
Creating another Agent in the same Sandbox with an optional GitHub URL passed:
the URL appeared in its first prompt and the Agent prepared the repository.
Stop showed accepted and confirmed progress. Delete hid the Sandbox immediately
and exposed optional cleanup progress. Original owner workload continuity
passed with the same Pod, PVC, Workspace and two Agent identities.

A spectator observed a 24.308-second disappearance of the conversation during
a transient access-check failure. The draft/history subsequently recovered,
but uninterrupted UI continuity failed. The next source retains an already
authorized personal view read-only during transient checks, retries stalled
Sandbox observations, and removes redundant Workspace adoption and historical
pool discovery from new native allocations. Those changes require a new
deployment and fresh timing; this release does not meet the full goal bar.

Private evidence is retained under `/tmp/cma-performance-release-publish-01`,
`/tmp/cma-performance-release-rollout-01`,
`/tmp/cma-agent-followup-live-PERF_3d6885c6_01`, and
`/tmp/cma-whole-home-PERF_3d6885c6_01`. Original workload comparison is
`/tmp/cma-performance-release-protected-after.json`. These files contain
operator evidence and are not repository artifacts.

## Combined performance and full-home release — PR #1016

Candidate `04ccbd6a` contains the native-home authority fix, faster durable
lifecycle progress and readiness observation, immediate Sandbox action feedback,
and full `/home` persistence for new native allocations. PR #1016 merged directly
into `develop`.

| Source and verification | Exact identity |
| --- | --- |
| Reviewed candidate | `04ccbd6a3adb9ec2c2f46ab9d2b01ae93c6f7b9f` |
| PR #1016 merge into `develop` | `fea975499c745febfcbf4bcfcaa71c3967835ca3` |
| Actual CI image checkout | `993591d35f937e26cdc3c2756a596dc4132e6282` |
| Candidate, merged `develop`, and image source tree | `b4c87d5d66897c3922d4da0768ac90bf2fe319d1` |
| CI | [38539408542](https://github.com/ChronoAIProject/cma/actions/runs/38539408542), attempt 1; full 15/15 jobs passed |

The completed publication receipt records these exact registry manifest digests.
They identify imported CI image bytes; Docker config image IDs remain separate
identities in the receipts.

| Published component | Registry manifest digest |
| --- | --- |
| Agent Manager | `sha256:d6242f4dae1ac07f26805216855331df3ef78233f5cb02986d780b9fc7ea0075` |
| Shared Web | `sha256:38f1c94685325465e4391b8aa0b1e9bd78cfae7367b8de921e6fddd6d103e43c` |
| Default runtime | `sha256:cad2058fad219bf8e6deb8b061a1d6331d28552990ee6bc8646f8cd5cf11d18b` |
| Event Gateway, retained | `sha256:0c3068130eaf618ce88dbe47bd7d111d365a5bba816c49f78b4000e8550ef1e7` |

API and Web use the established
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/` repositories
`cma-api` and `cma-web`; runtime uses
`asia-southeast1-docker.pkg.dev/chronoai-501608/chrono-pod/cma-runtime`.

| Combined release evidence | Receipt SHA-256 |
| --- | --- |
| `/tmp/cma-direct-home-v2-publish-01/publication-report.json` | `b06759305bb12851227a47310d19f5e188785f34481a4bff33e6c91bbf38937d` |
| `/tmp/cma-direct-home-v2-publish-01/import-receipts.jsonl` | `0773163de2c13e3a3306c2021fe33734bb2ad874487e2dc5fe09805655ca7a59` |
| `/tmp/cma-direct-home-v2-rollout-01/readiness.json` | `f3be18bcb55b5d1e8b8a33fe452d63cfc05015ddff98a18360e7db3922e5cce9` |
| `/tmp/cma-performance-home-v2-pool-direct-01/ready.json` | `79d1377a1bd56229020a535865a80d1afed1377183336aeb811db999342ef79f` |
| `/tmp/cma-agent-followup-live-FINAL_04cc_01/report.json` | `b5ce344ab796f7737179057f0505efbbe8411ccfce279661084c70a97a180f12` |

Publication proved the actual PR merge parents `[ab789bde, 04ccbd6a]`, equal
candidate/merge/CI trees, exact run attempt and artifact identities, runtime
execution evidence, and destination manifest/config readback. The repository
pins now match this completed publication.

At **18:02:50 SGT**, Agent Manager generation **16**, Web generation **92**,
and retained Gateway generation **6** each had one updated Ready replica, current
observed generation, the exact published imageID and zero restarts. The rollout
atomically changed the default runtime and home mode before restarting AM, then
updated Web. Gateway configuration and specification were retained.

At **18:04:03 SGT**, the new default pool had **two observed Ready replicas** at
1,000 millicores and 1,024 MiB each. Native template inspection confirmed the
exact runtime, `/home` mount with subPath `home`, full-home initializer and
matching pool/template UIDs. The receipt preserved nine extant native bindings
and the infra `python-a` pool/template; it recorded unrelated owner activity
without treating revision changes as a release failure. No old pool was resized.

A fresh actual UI creation at **18:04:18 SGT** used one creation request and
succeeded, automatically opening the Agent with an enabled composer. Browser
click-to-visible surface/composer time was **21.122 seconds**; the lifecycle
terminal state was observed at **19.818 seconds** and the admission response took
**159.22 ms**. These clocks measure different boundaries. The under-five-second
creation target **was not met**. The matching Agent/Sandbox/Workspace identities
and browser timing samples are retained in the creation receipt.

The same fresh fixture subsequently passed real inference with `gpt-6`,
`gpt-6-astra`, then `gpt-6`; all 15 eligible model options were enabled. Ornn
skill installation, exact request replay, and the next real turn reading the
installed skill passed. The scope-revision flow passed through a real Codex MCP
request, owner popup approval, delivered key generation, and exactly one
automatic notification in the original conversation; reload did not duplicate
the notification. The selected NyxID connection was already satisfied, so that
live run did not exercise a fresh external OAuth callback. Stop showed
Accepted/Waiting followed by Confirmed.

Delete then passed on that stopped test fixture. Click-to-submitting feedback
was **6.9 ms**, accepted feedback **289.8 ms**, and disappearance from active
navigation **289.9 ms**. Cleanup navigation and underlying progress began closed
and could be opened. The same request reached confirmed retirement in
**34.420 seconds**. Immediate user-visible removal met the target; the background
cleanup time is reported separately in `lifecycle-acceptance/delete-report.json`.

Pause UI feedback took **1.8 ms**, accepted feedback **175.9 ms**, and Agent
paused observation **1.547 seconds**; Sandbox completion took **8.797 seconds**.
Resume submitting feedback took **1.530 seconds**, accepted feedback **1.781 seconds**, Agent
ready observation **28.118 seconds**, and Sandbox completion **29.273 seconds**.
Default underlying progress stayed hidden. Neither actual Sandbox completion
met five seconds. The original helper incorrectly counted a stale "Sandbox
paused." banner as resume feedback and completion at 0.2 ms; the timings here
use the retained current-action UI events. The actual "Sandbox resumed."
message appeared at **28.948 seconds**. The replacement helper requires exact
current-action texts and a newly observed submission before UI completion.
The paused capture proved zero owned Pods and the retained
PVC; resume created a new Pod on the same PVC UID
`34a4b736-c503-402f-95ea-13a24def6a5c`.

Whole-home content and conversation checks passed across that Pod replacement:
15 entries covered `/home/cma`, `/home/.codex`, and a sibling directory,
including symlinks and staged, dirty and untracked Git state. Native conversation,
transcript and event prefixes survived, and a real recall turn returned the
earlier marker. **Strict filesystem persistence failed:** kubelet's default
fsGroup remount handling changed POSIX modes, including `0600` to `0660` and
`0750` to `0770`. No data was reseeded or permissions repaired to claim a pass.
New-policy mode preservation requires a new fixture after the follow-up rollout.

Evidence is retained under `/tmp/cma-agent-followup-live-FINAL_04cc_01/`:
`acceptance/models-durable-continuation-report.json`, `acceptance/skill-report.json`,
`scope-acceptance-02/report.json`, `acceptance/stop-report.json`, and
`lifecycle-acceptance/{pause,resume}-report.json`. Whole-home evidence and the
explicit failure report are under `/tmp/cma-whole-home-FINAL_04cc_01/`.

The release selects API, shared Web and runtime images while retaining Event
Gateway. One Web image implements all three portals; Image Manager backend
activation remains deferred. New native allocations use
`native_vct_full_home_v2`, mounting the PVC `home` subdirectory at `/home` and
running `--initialize-home-tree`. Existing `native_vct_empty_v1` allocations keep
their recorded image, original `/home/cma` mount, initializer and home authority.
The new default does not migrate existing volumes.

## Follow-up performance work awaiting production measurements

The adapter overlaps independent fresh native authority reads while preserving
Claim, Sandbox, Pod and PVC identity checks, policy validation, the final
resourceVersion compare-and-swap, and post-resume home checks. Contextual resume
uses the recorded home to render both generations after one graph proof. The
focused no-explicit-policy transition now performs seven GETs and one PATCH,
compared with twelve GETs and one PATCH before this change. These are request
counts established by controlled tests, not measured production latency.

Initial Agent connection publication reuses its exact locally staged conversation
when the durable open receipt matches and no settings need replay. Missing local
state still reconnects the recorded conversation. Update activation carries the
current publication authority; a stream lost during publication retains durable
recovery work. This removes the redundant second conversation open from normal
startup without creating another conversation or weakening restart recovery.

Paused, pausing and resuming Sandboxes retain navigation to their existing
Workspaces and Agents. History and drafts remain visible. Runtime actions and
message input follow the actual lifecycle state; an accepted resume does not
advertise a ready runtime. Detailed underlying progress remains optional.

The retained production resume sample spent approximately ten of its eleven
Pod-startup seconds before containers started. Its initializer completed in the
same recorded second, and the runtime became Ready in the second it started.
The readiness probe has zero initial delay and a two-second period. There is no
evidence from that sample that reducing probe delays would remove the dominant
wait. The new default OnRootMismatch policy avoids recursive fsGroup remount
processing when the volume root already has the expected ownership and mode;
its permission preservation and timing must be measured on a fresh allocation.
The original sample has no retained events sufficient to separate volume,
network and sandbox setup time, so that wait is not attributed solely to fsGroup.

## Prior Agent follow-up — 16:26:14 SGT

Before this combined release, candidate `03cbd332` was Ready at **16:26:14 SGT on
2026-10-14**. Agent Manager generation 15, Web generation 91, and retained Gateway
generation 6 each had one updated Ready replica, exact published imageID and zero
restarts. The default runtime was `a955564d`. PR #1007 merged as `ab789bde`, with
actual CI image checkout `89fc1aaef7d71b80200f8e6af9670738c843e029` and equal tree
`7312f2b7bad364092fbb8a05d80d9dbffb1c9685`. CI run
[38528498008](https://github.com/ChronoAIProject/cma/actions/runs/38528498008),
attempt 1, passed all 15 jobs.

| Prior release evidence | Receipt SHA-256 |
| --- | --- |
| `/tmp/cma-agent-followup-publish-04/publication-report.json` | `45cb6e81fb1ce3ae48bc6600b289944fe03fc3706d519a4abc5b228bf33d88c9` |
| `/tmp/cma-agent-followup-rollout-01/readiness.json` | `73b5bbafa878d20c8e2d2af8d669779c1d7d301ac49d0959aa80b673eaac5a62` |
| `/tmp/cma-agent-followup-pool-01/ready.json` | `f652d6733a06a1a468512901ca98df60a4985ea8004f3999f0a876c69f8860fa` |

The pool receipt observed two Ready replicas at 16:26:50 SGT. A later owner
attempt reported **Needs attention** during Sandbox preparation. These prior
readiness receipts do not establish successful fresh Agent timing or live skill
installation. The combined release includes the authority correction; its live
outcome requires separate evidence. Receipt paths are operator-local artifacts.

## Earlier verified baseline — 11:31:31 SGT

At **11:31:31 SGT**, Agent Manager, Web and Event Gateway each had one updated
Ready replica, current observed generation, the exact published imageID and
zero restarts. CMA runs in northeast `chrono-bot`; native Sandboxes run in
southeast `chrono-pod`. One Web image implements all three portals. Image Manager
activation remains deferred.

| Component | Candidate | Registry manifest digest |
| --- | --- | --- |
| Agent Manager | `cd37968d` | `sha256:b641e4e1795acac4e0586ac4db01ba43d55b3e2c7d3d1bea5b0e54b2b5f99eef` |
| Default runtime | `37d6f95c` | `sha256:1c44be8aaa7b88c0190d17ca44f4f6e0f9a8aec880038c5d4da9cb2bc8c68e8b` |
| Web | `ac837aa7` | `sha256:477215d347f052c3315a8dafc3be31e89a6c4516772f65bc21e8aee83b884a0f` |
| Event Gateway | `cabfe5c3` | `sha256:0c3068130eaf618ce88dbe47bd7d111d365a5bba816c49f78b4000e8550ef1e7` |

Application images use
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/` repositories
`cma-api`, `cma-web` and `cma-cmaeg`. Runtime uses
`asia-southeast1-docker.pkg.dev/chronoai-501608/chrono-pod/cma-runtime`.
The following are actual CI image source revisions, not inferred PR merge SHAs.

| Component | Actual image source revision | Source tree | CI |
| --- | --- | --- | --- |
| AM | `b8f6ae10d048cd95593ef9efb88ff88195d82216` | `73a43035bda9d9f07e38ac378ab9eb44dfc2e17f` | 38502541842 attempt 1; full 15/15 passed |
| Runtime | `1d2473a3e7f8e5961f75b63725ebc0742f973fe7` | `b84818d8dbbe22ee84ebd575aec0cb94c650e7fa` | 38482643832; full 15/15 passed |
| Web | `d498b1b5e5541485654b30d9b5c97f80ae3e0962` | `f99e9ae871af0b291d657c71f1ddc8db637d759d` | 38504776234 attempt 1; four required jobs passed |
| Gateway | `21f3401204becb767e462bf5fb4258d996dc923d` | `aae602e75dc85ff5f8cb7aabb14c8504aef619ea` | 38496119196; full 15/15 passed |

AM publication required Backend, Images(api), Repository structure, Secret scanning
and Manifests. Its full run was in progress at publication, then completed 15/15
at the 11:32:56 SGT observation. Web required Images(web), Frontend, Repository
structure and Secret scanning; its full run was in progress at publication.
Scoped publication and full-run observations are separate evidence. At the
11:47:00 SGT check, Web had 13 successful jobs of 14 created, with Backend still
running; no full Web CI success is claimed. Current snapshot:
`/tmp/cma-scope-text-01/full-ci-current-observation.json`.

| Release evidence | Receipt |
| --- | --- |
| AM exact publication and CI observations | `/tmp/cma-native-conflict-01/publication-report.json`; `full-ci-at-publication.json`; `full-ci-final.json` in that directory |
| Web exact publication and publication-time CI | `/tmp/cma-scope-text-01/publication-report.json`; `full-ci-at-publication.json` in that directory |
| Gateway exact publication | `/tmp/cma-test-timeout-01/publication-report.json` |
| Runtime publication, residue scan and canary | `/tmp/cma-release-final-37d6f95c-01/publication-report.json`; `/tmp/cma-release-final-37d6f95c-01/runtime-residue-scan.json`; `/tmp/cma-runtime-final-37d6f95c-01/result.json` |
| Final complete rollout/configuration | `/tmp/cma-rollout-native-conflict-01/plan.json`; `/tmp/cma-rollout-native-conflict-01/readiness.json` |
| Public and owner readback | `/tmp/cma-public-endpoints-final.json` |
| Synchronized repository manifests | `/tmp/cma-final-repo-sync-01/resolved-receipts.json` |

AM generation 13 uses Pod UID `b65b0274-3d86-40e3-8c0c-5e7fba98a910`.
Web generation 89 retained UID `1ea4ec73-3628-4dd7-800a-6ed6eb9dc430`;
Gateway generation 6 retained UID `2de69466-5794-4ea5-9ff0-a5ba584ffddc`.
Deployment UIDs are unchanged. AM remains singleton/Recreate with native
agent-sandbox enabled and authenticated `kubernetes-exec` transport. Runtime 37d
and Gateway origin `http://cma-api.chrono-bot.svc` are preserved. No infra-owned
controller, CRD, RBAC, storage or network policy was changed.

At **11:31:59 SGT**, all seven public/owner checks returned HTTP 200: Web `/`, AM
`/ready`, Gateway `/readyz`, authenticated non-visitor owner session, valid owner
inventory, and the original protected Sandbox/Agent, both still paused.

## Earlier measured acceptance

| Flow and tested version | Actual result | Evidence |
| --- | --- | --- |
| Three creation contexts — AM/runtime 37d | New Sandbox, existing Sandbox and existing Workspace each preserved a 79-byte UTF-8 title, empty workspace, `gpt-6/high/autonomous`, mandatory scopes and target identity. Each completed a real first turn; identical operation replay returned 202 with the same operation. Native imageID matched runtime 37d. | `/tmp/cma-final-owner-new-37d6f95c.json`; `/tmp/cma-final-owner-existing-sandbox-37d6f95c.json`; `/tmp/cma-final-owner-existing-workspace-37d6f95c.json` |
| Persistent home/history — AM/runtime 37d | Native canary passed assignment/replay and same-PVC suspend/resume. API exports were identical UTF-8 JSONL, 43,428 bytes and 20 records, across five requests. Retained Mongo identities/history prefixes and protected original history survived rollout. | `/tmp/cma-final-history-export-37d-zo5ye6_a/report.json`; `/tmp/cma-final-rollout-after-37d6f95c-zht4aiai/reviewed.json`; `/tmp/cma-secondary-model-resume.json` |
| Owner Details/settings/model/history — Web c63, AM 37d | Details exposed only model, reasoning, permission mode and key scope, with both mandatory scopes fixed. Three changes/restores returned 200, runtime acknowledgment, reload persistence and stored-event replay. Two actual turns used native-confirmed gpt-5.6-sol and gpt-6. Two menu downloads matched independent HTTP export: 98,511 bytes, 46 records. | `/tmp/cma-final-agent-ui-proof-FigMWZ/acceptance-reconciled.json`; `/tmp/cma-final-agent-ui-proof-FigMWZ/native-model-proof.json` |
| Final service-picker appearance — Web ac837 | At 11:20:13 SGT, 1442 and 390 px checks each measured 105 service rows/431 elements with zero overflow. Screenshots passed review. Viewport restored; Agent/Sandbox stayed paused at revision 85; zero revives, settings changes, prompts or unexpected mutations. | `/tmp/cma-final-details-style-proof-kHhQRd/report.json` |
| Owner-approved MCP continuation across AM replacement — AM 37d/Web 5ce, retained runtime 028aca | One owner approval returned 200; exact connections were ready; credential generation advanced 2 to 3. Original conversation automatically completed one acknowledgment at turn 56, with no new prompt and preserved prefix/reload. | `/tmp/cma-final-warm-approval-proof-xwfmHa/report.json`; `/tmp/cma-final-warm-approval-resume.json` |
| Earlier portal/runtime flows — versions in individual receipts | Browser creation and optional repository first prompt passed. Pause/resume retained home/conversation identities. Warm-pool adoption used its existing native resources; resize to zero preserved the claimed Agent/PVC, rejected stale revision with 409 and removed unclaimed resources. | `/tmp/cma-real-portal-owner-proof-j9MtV6/report.json`; `/tmp/cma-optional-repo-f034/status-redacted.json`; `/tmp/cma-native-live-accept-f034/`; `/tmp/cma-native-warm-live-8ur5fbbq/report.json` |
| Hosted callback — already-connected service | Registered return URL and official hosted completion passed; temporary binding removed and all 47 original bindings preserved. This was not a fresh missing-service OAuth journey. | `/tmp/cma-fixed-connection-callback-client-receipt.json`; `/tmp/cma-real-hosted-callback-proof-xG2iZH/report.json` |
| Earlier Gateway routing — versions in individual receipts | Independent OIDC login, target-preserving edit, selected-Agent management test and custom-event ingress completed actual model responses. Original Telegram channel preserved. An earlier explicit Busy response admitted zero turns; a caller retry completed one acknowledgment. | `/tmp/cma-real-gateway-ingress-Vk2ucc/report.json`; `/tmp/cma-gateway-inline-live-Gzcyq5/report.json` |
| Final cold inline-Agent Gateway — AM cd379/Web ac837/Gateway cabfe | Passed at 11:43:56 SGT: zero pre-test Agents; one actual UI click/POST returned HTTP 200 with the exact acknowledgment visible after 42.624 seconds. One ready Agent, one completed turn, one user/assistant pair and 19 contiguous events; reported acceptance latency 97 ms | `/tmp/cma-gateway-cold-live-czmxw4jd/acceptance-receipt.json` |

The c63 functional receipt reconciles an initial empty CDP body capture using
actual downloads and independent HTTP, without repeating settings or prompts.
It proves successful stored-event replay, not a fresh refused-settings flow.
The earlier caller retry and ready-Agent Gateway checks do not substitute for
the final cold management-test result.

## Earlier cleanup and protected resources

| Fixture | Normal cleanup result | Receipt |
| --- | --- | --- |
| Retained warm permission fixture | DELETE 202; revocation event 71; exact Claim/Sandbox/Pod/PVC absent at 09:24:45 SGT. Disabled application pool target/ready stayed zero; application/native and infra python-a pool UIDs preserved. | `/tmp/cma-warm-retained-cleanup-report.json` |
| Failed 5ce cold fixture | Original DELETE 202; Sandbox/Agent retired; exact four native resources/channel absent and dedicated key revoked by 09:32:19 SGT. Lease/home races converged without extra mutations or forced deletes. | `/tmp/cma-gateway-cold-live-lm2syu_a/owner-retire-receipt.json` |
| AM 37d provisioning-failure fixture | Normal channel DELETE 204 retired the Sandbox; channel/key and four exact original native resources absent by 10:36:34 SGT. | `/tmp/cma-gateway-cold-live-14esj40l/failed-provision-native-exact-absence.json` |
| Primary paused acceptance fixture — AM 37d | DELETE and same-key replay returned 202; retired revision 98; credential event 78 completed. Original Claim/Sandbox/PVC absent at 11:21:16 SGT; Pod already absent while paused. One transient stop_agents failure converged normally; no forced delete. | `/tmp/cma-final-primary-paused-cleanup.json` |
| Final cold fixture | Passed by 11:45:14 SGT: channel DELETE 204 then 404; Agent deleted at 11:44:29 and Sandbox retired at 11:45:06; all four original native resource UIDs absent, dedicated key revoked/404, disposable row absent after reload | `/tmp/cma-gateway-cold-live-czmxw4jd/acceptance-receipt.json` |

The protected original Sandbox and Agent are distinct from these owned fixtures;
the final seven-check public readback confirmed both remained paused.

## Corrections and relevant historical failures

| Earlier evidence | Failure and final correction | Original receipt |
| --- | --- | --- |
| Runtime 2bc6 | Build-test installation residue caused a Sandbox identity mismatch. Final 37d isolates build state, honors persistent TMPDIR and passed a zero-residue scan across 15 logical layers plus native canary/application flows. | `/tmp/cma-release-final-37d6f95c-01/runtime-residue-scan.json`; original diagnosis retained in [deployment handoff](CMA_Deployment_Readiness.md) |
| Settings event compatibility before 5ce | Missing TypeScript update_settings parsing hid Agent pages/approval. Regressions reproduced it; final source includes the parser and c63 live stored-success replay passed. | `/tmp/cma-paused-page-error-evidence.json`; `/tmp/cma-settings-event-compat-receipt.json` |
| 5ce cold management test | HTTP 503 provider_unavailable, zero admitted input/turns. Gateway cabfe separates 90-second startup from a fresh 30-second acceptance/completion budget; Web/default SDK callers allow 125 seconds. Only explicit Busy retries the same body/key; ingress remains 660 seconds. | `/tmp/cma-gateway-cold-live-lm2syu_a/report.json` |
| AM 37d native provisioning | Kubernetes 409 on the first home-authority patch caused terminal validation; zero Workspace/Agent creations and zero test POSTs. Final AM retries only exact 409 Conflict after rereading/revalidating original identities; AlreadyExists/unknown 409/ownership/UID/spec refusals remain terminal. | `/tmp/cma-gateway-cold-live-14esj40l/provision-failure-receipt.json`; `/tmp/cma-native-home-conflict-receipt.json` |
| Web e189/af1 picker visuals | e189 loaded styles but overflowed the grid. af1 fixed the grid yet a real 278 px unbreakable label still overflowed a 257 px field. Final ac837 scopes min-width:0/overflow-wrap:anywhere and passed actual desktop/mobile production checks. | `/tmp/cma-final-details-style-proof-FI8Ptt/report.json`; `/tmp/cma-final-details-style-proof-5qXLnB/report.json` |

Focused verification also passed the 48 KiB history regression, 28 native
transport checks and strict Clippy; four pinned ACP suites covered seven
settings/model/reload/credential cases. Those ACP suites used an isolated Pod
based on the prior runtime, separately from final-image acceptance. Final AM
conflict tests covered fresh versions, identical UIDs, no duplicate allocation/
PVC writes and replacement-UID refusal. Final Web local checks reproduced the
actual catalogue failure and passed 429 elements at 1442/1280/390/320 px plus
extra unbroken text. Receipts: `/tmp/cma-full-pinned-acp-receipt.json`,
`/tmp/cma-native-home-conflict-receipt.json`,
`/tmp/cma-details-scope-text-receipt.json`. Component CI results are above.

## Coverage limits

Image Manager build/catalog activation, fresh missing-service OAuth, NyxID
frontend automatic navigation/binding-key delivery, external reply sinks,
broad process-crash/failover, full Console/media/STT, Trigger delivery and
cross-user sharing are not established here. Existing Agent UI uses AG-UI; no
production UI transport-flag switch is claimed. Runtime canary results use
operator identity and local status; real application flows are separately
identified. Exec connectivity does not prove direct Pod-network isolation.
A default runtime update does not migrate retained Sandboxes; the MCP approval
flow intentionally used retained runtime 028aca.

The earlier four-manifest synchronization and its YAML, configuration
key/documentation, native contract and route ownership checks apply to the
versions identified above. Operational instructions and infra ownership boundaries are in
[CMA deployment readiness](CMA_Deployment_Readiness.md).
