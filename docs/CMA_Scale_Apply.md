# CMA scale: staged apply and rollback

Runtime stream/request mapping and the **default-neutral** #1487 activation
profile are specified in [Runtime capacity](CMA_Runtime_Capacity.md). With the
current HPA max 4 and 48 usable streams/process, **200-active N+1 is not met**;
replica shape or a higher qualified per-process limit remains an owner decision.

For the lead's **15 November 17:23 SGT WS5 instruction**, use the
[EG HPA production handoff](operations/eg-hpa-production-20261115.md): EG min2/max4
with S2 before AM S1. The controller workflows below retain their earlier
sequencing constraints; they are not additional prerequisites for that handoff.

Operational handoff for **deployment-engineer-01/02**, issue #1956. This PR targets
develop **after #1370**. It is an inert, conditional manifest delivery; neither
merge nor this document authorizes production writes. Application switches remain
default-off in the integration. Follow architect A2
(`b238fbbc032f2f6d8bdfd96d8bbc669a985833a2a1cd1ca153f46d5c9ec7c717`),
updated contract (`79b2de77e6b4b68a1c919ba68b355487274787ad702ef111766313c22840b11f`)
and stages (`229cc0388034fed1d102da641802a0789e469650ae0d1bd20cf1297ce4cf4909`).
Read [capacity qualification](CMA_Scale_Capacity.md) before any stage.
The canonical-controller preparation described below is **execution HOLD** until
its exact tooling and isolated actual-controller proof receive architect PASS.

## Source of truth and execution authority

AM manifests are `infra/api/{deployment,pdb,hpa}.yaml`; EG manifests are
`infra/cmaeg/{deployment,pdb,hpa}.yaml`. Release publication does not apply them:
`docs/Release.md` defines explicit promotion; workflows validate/package images.
Live image pins can be newer than checked-in Deployment pins. **Never apply an
entire Deployment or directory for this change.** The sole execution path is the guarded controller in the
[common operator setup](notes/reviews/2026-11-15/deployment-runbooks.md),
[S1 commands](notes/reviews/2026-11-15/s1-execution.md),
[S2 commands](notes/reviews/2026-11-15/s2-execution.md) and
[S3 commands](notes/reviews/2026-11-15/s3-execution.md).
Those operations use guarded field patches and absence-guarded PDB/HPA creation;
full-manifest apply and the former direct mutation examples are retired. PDB `unhealthyPodEvictionPolicy: AlwaysAllow`
lets a non-Ready pod be evicted without indefinitely blocking node drain;
`minAvailable:1` protects the healthy survivor. Both topology constraints use
`matchLabelKeys: [pod-template-hash]` to count each rollout revision separately.
With service-only counting, old[0,1]+new[1,0] can admit another new pod on the
first host (total[2,1]); removing old leaves new[2,0]. Require server admission
and actual rollout verification of distinct hosts after every old pod is gone;
schema validation alone does not prove scheduling. The local F001 observation
and this admissibility example do not establish its historical scheduler trace.

Lead admin-merges reviewed exact heads using `--match-head-commit`; the team
builds locally under approved build authority, without waiting for/polling CI.
Before each gated write, deployment engineers reread
`team.sh policy show ChronoAIProject/cma` and consume a fresh
`team.sh policy-check deploy --approval <step-id> --context <release-SHA>` grant.
The selected route is `reviewed-release-controller`: use only the linked
`plan_scale`/`apply_scale` operations, each with its printed operation-bound
approval context. A refusal stops the step; never edit policy or substitute a
raw mutation command.
Obtain explicit rollback permissions before starting. Server dry-run does not
replace permission, application tests or live backend verification.

The binding 14 Nov 20:30 [timeline](notes/reviews/2026-11-15/stage-timeline.md)
plans Batch9 at 14 Nov 21:00 (N=1, shipped defaults, 15-minute checkpoint and
auto-rollback), S1 approximately 22:30 after checkpoint and #2043 dry-run PASS,
S2 approximately 15 Nov 00:30 fully soaked before 02:00, and S3 approximately
05:30–06:00 (quota 64/128 → HPA → S3b once #2038 merges). Log01's Batch9 S1
baseline is 21:30–22:30. Activity → U1 → U2 begins 03:00; U2 starts before 04:30
or after actual S3 success. U3 needs `pass_complete` and at least 15 minutes
from every confirmed S-stage start. M8 uses prod-verifier01's kit 09:00–16:00;
final report remains due 15 Nov 18:00. Each stage needs observed prior success
and a fresh lead grant. On failure, roll back compatibly, report and let the
lead re-slot. No writes 02:00–02:59, including recovery; preserve the controller's
01:58 margin and never begin a bounded write that can cross the blackout.

## Release packet and local preparation

The lead supplies exact integrated source and manifest SHAs, architect PASS
for every relevant head, locally built immutable AM/EG/runtime image digests,
build provenance, additive schema/role receipts, retained-runtime image/protocol
and exact challenge/lease-duration compatibility floor,
all local correctness/performance/fault evidence, approved max-owned-Agent value,
capacity receipts, fixed HPA cadence and per-stage/per-step approvals. No moving
tag or invented digest is acceptable. An unresolved item is HOLD.

From the exact approved checkout, follow the linked common operator setup and
populate its release input from qualified source/image/capacity evidence. The
controller creates each narrow plan from fresh state, performs a server dry-run,
and binds the resulting plan to one approval. Its allowlisted transformations
preserve unrelated environment, resources and template fields. The former Ruby
patch renderer and local mutation helpers are not an execution route.
All git/gh commands require active `chronoai-shining`; do not switch.

EG address and switch names are developer-15's source contract pending exact
integrated-SHA review. Confirm the existing EG listen port8090 (`CMAEG_LISTEN` and named `http`
container port), direct Mongo mode,
additive `--migrate-replicas` marker/index and durable routed admission/recovery
before S2. The advertised address must be a literal Pod IP/direct origin, never
a Service address. Neither this patch nor drain204 proves sockets/leases handed
off. Reconcile any final reviewed binary contract change before sealing the packet.

Existing named env entries must have the expected source/type. Preparation
merges topology constraints by `(topologyKey, whenUnsatisfiable)`, preserves
unrelated entries and fields, and replaces only `lifecycle.preStop`, retaining
`postStart`. Duplicate topology identities or unrecognized lifecycle shapes
refuse preparation. Saved previous fields retain their exact presence/value for
pre-stop restoration. The controller tests the fresh Deployment UID, resource
version, generation and full spec immediately before each guarded patch. Drift
requires inspection and a new plan; read back every mutation.

Before any write, capture only images, replica/status counts, strategy, resources,
probes, grace/hooks, topology, named nonsecret replica controls, resourceVersion
and existing HPA/PDB specs into private receipts. Hash pod identities and record
zone/counts; no Secret reads, raw ConfigMap/Deployment exports, pod exec or logs
with request/user payloads. Keep source/image/config changes as separate strata.
Do not restore an old whole object from this capture.

## Common stage gates and verification

**Latest lead M3 merge ruling:** local source/resource placement, PDB and
readiness subchecks PASS; retained f6 host cross-ingress N1/N2 OFF/ON PASS;
all integration-head (5cfd or newer) multi-instance suites against one real
Mongo must report PASS/FAIL through local-verifier-02; environment01 EG fixture
cases PASS. The suites cover killed-owner takeover, recovery leases, fencing,
no duplicate effects, title custody, desktop exclusive sessions and EG delivery/SSE.
Architect04 supplies the quick integrated source pass. This supersedes the
earlier requirement for a deployed local AM runtime/auth/provider workload before
merge. Unrun local end-to-end rows remain unrun; S1 observes real production
accepted work as specified below. The missing local adapter is not an M3 merge blocker.

Require integrated local fmt/clippy/tests and real Mongo independent-instance
proof. **Before zeroing legacy AM**, inventory every retained traffic-capable
native/hosted runtime and verify its exact image digest, protocol and ownership
challenge/lease-duration compatibility with the final candidate. Protocolv1
presence alone is insufficient: old #1370 receivers use20s while the integrated
target is Mongo15s/renew3s and receiver10s; the receiver challenge `valid_for_ms`
must exactly equal the manager's tested expected value (proposed10000ms).
Pin the final durations and compatibility matrix in the release packet.
Require positive same-floor evidence and negative tests rejecting an old20s
receiver that still advertises v1, wrong challenge duration, stale/expired
challenge and incompatible image/reader combinations. Those integration tests
are mandatory evidence, **not executed by the manifest validator**. Any missing
runtime, stale inventory or failed/missing challenge proof blocks S1 before
the first scale-to-zero. Updating only the default runtime image does not qualify
retained runtimes or replace their upgrade/continuity proof. Then require:
shared auth/tickets/desktop/quota/cache and every background effect is safe at N;
AM101 sudden-loss trials, EG10 process/node losses and two worker saturation/panic
tests meet <=30s sustained service recovery. Local kill plus cordon/rescheduling
node-loss equivalent is required; a production drill is optional after S3 with
separate approval. Supported provider idempotency must be used; without it never
automatically resend Unknown/Partial. Preserve flagged intent/custody, stop
promotion and page the lead. Every accepted S2 canary must reach terminal or
reconciled state; unresolved flags are not PASS. Reconciled failures stay failures.
A2 replaces pre-release24h/1000 events with provider-specific S2 canary soak and
assigned24h post-go-live monitoring, including owners, approved mechanism and
abort/recovery triggers in the packet. Preserve user homes,
accepted work, epoch watermarks and original operation IDs. No sticky affinity.

Require matched same-candidate N1/N2/2→3→5→2 performance and the frozen batch2/3
route catalog, plus final-candidate N1 versus the latency owner's baseline.
Frozen `latency-route-catalog-a2.json` SHA256
`a47475609633f3f736147dbded6dace5864d174b0cf58283425e639aba3ad951` has43 criterion
rows,37 route memberships/34 distinct routes and six client boundaries. Preserve
every repeated/conditional/alternate row; missing routes remain INCONCLUSIVE.
Plain/desktop Agent create <=5s END TO END; tab-open/send/interrupt <=1s, usage
<=2s. Header timing cannot replace any completed client journey.
Use at least three baseline repeats, preregister measured noise and confidence
criteria before N2; report N, errors, missing completions, p50/p95 and p99 only
when N>=1000. Missing/unstable data is INCONCLUSIVE. Do not generate production
canary traffic or faults without separately scoped authorization. Analyst02's
final-candidate baseline packet must be rebound to the revised timeline.
Log01 supplies the 60-minute 21:30–22:30 Batch9 N1 baseline and retains the standing
24h context; the existing 1800-second synthetic READ session is a separate bound
window. Superseded morning windows do not establish matching coverage. Preserve
actual counts, failures and missing boundaries without fabricating extra windows
or compressing the statistical method. N1 modeOFF versus N2 modeON remains a
configuration confounder, with the existing statistical/policy distinction.

Read-only resource checks, after the linked common operator setup:

```bash
k get deployment cma-agent-manager cma-event-gateway \
  -o custom-columns=SERVICE:.metadata.name,DESIRED:.spec.replicas,READY:.status.readyReplicas,AVAILABLE:.status.availableReplicas
k get hpa,pdb,resourcequota,limitrange
kubectl --context "$scale_context" get apiservice v1beta1.metrics.k8s.io
k top pod -l 'app.kubernetes.io/name in (cma-agent-manager,cma-event-gateway)' --containers
```

Use each stage's `plan_scale` for server dry-run of its exact guarded operation.
These dry-runs do not establish membership/capacity. Prove the AM image-contained
`/usr/local/bin/cma --agent-manager-drain` hook under non-root/read-only filesystem:
authenticated loopback, idempotent, <=25s, early irreversible readiness withdrawal
and close-before-exact-lease-release. Its shell waits30s and returns the CLI status.
EG retains native Kubernetes sleep30 before SIGTERM: an exec hook inherits Secret
variables in a dumpable process visible to same-uid ffmpeg jobs. S2 remains blocked
until a reviewed credential-safe early-drain boundary is implemented and qualified;
do not reintroduce the shell/CLI hook or treat sleep as proof of early owned drain.
SIGTERM must perform safe drain and failed close must retain authority until expiry.
Neither grace nor a healthy probe proves failover performance. Budget
AM25+30+660+30+15=760; EG reserves at least30+660+45 within760. Prove actual shutdown
fits the grace; these sums are not measured process/client lifetime bounds.

## Controller and route packet: prepare before the outage

Retain each canonical Deployment UID/selector, four Service aliases, Gateway,
HTTPRoute, NEG identity and backend policies. No routing, affinity,
backend drain or health-check change is proposed. The only temporary HPA-target
exception is the separately granted S3b exercise linked below. Include exact Route parents,
backendRefs/ports, Service selectors/targetPorts, endpoint/NEG membership and
backend health source in the lead-approved packet. Console/legacy-console are
reachable EG routes. GCP backendServices list permission was denied to the
engineer: obtain an authorized observer get-health receipt, not a permission bypass.

Architect-proposed supported-controller variant, **pending lead approval of a
change to A2's literal pre-created-RS0 order**: no HPA/concurrent release; pause
canonical Deployment at1 and verify stable old1; install the final template at
canonical desired1 with replica mode confined to new pod-template env. Pre-pull
immutable images and qualify nodes before the outage. Then old1->0; prove every
legacy effect stopped; unpause at desired0 so the Deployment controller creates
the exact owned newRS0; verify it before scaling canonical desired2. RS0 therefore
appears **after** old shutdown. Prepared template/cache is not prepared RS0 or
a running warm application. Do not claim the literal earlier order is satisfied.

Do not manufacture ReplicaSets, pod-template-hash, ownerReferences or revisions.
The actual Deployment controller must create/own/select the new RS; verify exact
parent UID, selector, template, image/configuration and desired0 with no legacy
restart or orphan adoption. Pausing prevents template rollout, not all scaling.
A separate candidate Deployment changes routing/HPA ownership and needs its own
review. If literal pre-existing RS0 remains mandatory, HOLD for separately
reviewed controller tooling/proof; none is supplied here.

Before production preparation, isolated actual-controller tests must prove
pause/template reconciliation at old1, stale UID/RV/concurrent-writer refusal,
controller restart, old1->0, unpause-at0/newRS0 and new0->2. Prove no legacy pod
can reappear and no candidate starts before the no-legacy-effects gate. API
server dry-run alone cannot prove this. Capture only narrow nonsecret fields;
never persist a whole raw Deployment/Secret to construct a template.

Use the linked S1/S2 pause and prepare operations. Before pause, the controller
saves only the original Deployment/ReplicaSet/Pod UIDs, template identity and
container IDs, restart counts, start times and image IDs. It rechecks that
incarnation before pause and before pre-stop restoration, including after the
approval call. A same-name recreated Deployment, replacement RS/Pod or restarted
container refuses the pre-stop branch. Old plans without the saved identity
cannot be used; never capture a replacement process as the original.

Preparation must leave only the original serving process, the controller paused
at one, and the new template inactive. It does not establish prepared RS0.

## S1/S2: proposed controller-owned old1->0, new0->2

S1 requires the complete retained-receiver exact image/challenge-duration floor
and negative tests **before** zeroing AM; EG stays at its proven state. S2 needs
S1 PASS plus every enabled provider/partition/console path's custody, shared
session/CSRF/refresh/logout and close-before-release proof. Both require explicit
owner outage approval for the current stage window, source/image/cache/node readiness and
an exact compatible recovery branch. The pending order exception above must be
approved first; a zero-sized RS contains no running warm application.

Follow the linked S1/S2 guarded stop, unpause-zero and start-two operations.
After old1→0, prove all legacy processes/sockets stopped or their authority was
safely revoked/expired. Pod deletion alone does not prove this. Only that proof
permits unpause at zero and exact controller-owned RS0 verification, then start
at two. Never force-delete or manufacture an RS to shorten the interval.

Measure from loss of the last healthy legacy backend to restored successful
user service; record both-new-pods-ready separately. Target **<120s outage**.
At120s without restored service, abort promotion into the preapproved compatible
recovery branch. Do not wait35m for rollout then discover the missed outage gate.
An old process that cannot safely stop in time is a preflight failure; deadlines
never shorten leases/grace. Once new authority has been used, normal recovery
retains modeON and compatible image/receiver floor. No automatic legacy restart.

After **both** new pods/backends qualify, use that stage's guarded `s1-pdb` or
`s2-pdb` creation operation. It refuses an existing same-name object and verifies
the stable pair before creation; reconcile a name conflict before proceeding.

Require readiness false before joined/renewed membership and immediately on drain;
liveness is separate. Each replica must independently serve; an idle EG member
may be ready without forced equal ownership. AM/EG CLI success is not proof of
all-owner handoff. Run the shared30-minute stable soak, then obtain lead S1/S2
PASS. S2 also requires provider canaries: every accepted event terminal/reconciled,
no unsupported blind replay, and the assigned24h observation handoff. No ON1
intermediate promotion gate is part of this cutover.

## S3: HPA activation and measured automatic up/down

For paired S3, require S1/S2 PASS, verified Atlas>=30% headroom, the full quota/node/provider bound
and CPU qualification in the capacity document. The selected cadence and max5
must match reviewed tests. No rollout overlaps HPA qualification. Existing HPA
must be absent; otherwise capture/review its exact ownership before proceeding.

The release packet binds complete uncensored successful cohorts and the largest
qualified p95 from drain initiation through all-owner handoff AND successor
service. Missing timing HOLDs S3; p95 is never the hard client lifetime bound.
Scale-down is fixed at **one Pod per 120 seconds**, with **300 seconds** of
stabilization. Unknown p95 or p95>90s holds S3; never widen the interval. Both
templates and every actual Pod must have **760 seconds** of termination grace,
including preStop, before S3. Grace is not a measured hard process/socket bound.

Use the guarded S3 quota raise (CPU requests40→64, limits80→128 with saved
before-spec), then the separate AM/EG HPA creation plans and approvals in the
linked S3 runbook. The AM template is min2/max5; the EG template is min2/max4.
Both retain up0 Max(+2,+100%)/60s and down300 -1/120s.
Creation requires absence, fresh metrics/capacity qualification and stable
pairs; server dry-run and quota admission alone do not prove runtime capacity.

The lead-selected [S3b scale/drain exercise](notes/reviews/2026-11-15/s3b-execution.md)
uses existing real traffic and sequential guarded AM/EG target lower/restore
operations. It changes no Deployment replica count directly and keeps the
original scaling policy. Report observed transition/drain evidence separately
from useful-load/max-five qualification; the exercise cannot supply those claims.

Using separately approved load, observe real metric-triggered2→4→5 for each
(Max of+2/+100% per60s, up stabilization0),
then together after capacity proof. A manual replicas patch does not prove HPA.
Independent correctness also covers N3. Reduce only owned load; observe the
full300s down stabilization and exactly -1 per120s,
all Ready/terminating/live-client counts, successful early drain and handoff,
until stable two. Demonstrate no missing-metric downscale in isolated tests.
Start the final30-minute soak only after actual terminating processes are gone.
Do not reapply Deployment `replicas:2` while an HPA owns its scale subresource.

## Soak, abort, rollback and receipts

### EG-only HPA before S1 (Track A)

The separately approved EG-only route permits healthy AM1/OFF with S2 PASS and
EG2/ON. It does not clear S1 or paired S3b. Use the existing controller `plan`
and `apply` interface with [the EG-only packet example](../.github/scripts/cma_scale/eg-only.release.example.json).
The example refuses until populated with actual evidence; every write still
needs its own exact-context lead grant through the reviewed controller.

The packet's `eg_only_hpa.s2` records the completed >=1800-second S2 PASS,
matching release source/images, evidence SHA256 and exact Deployment bindings
(`uid_sha256`, `generation`, `spec_sha256`). Its `capacity` records a <=120-second
observation, evidence SHA256 and all listed qualifications. Budget at least
`5 + ceil(hard_lifetime_seconds / 120)` EG processes (12 at760s), plus AM and
other consumers, all live/terminating Mongo clients and >=30% server headroom.
The qualified hard lifetime must be >=actual760s; p95 remains <=90s.

| Controller action | Required prior result / effect |
| --- | --- |
| `s3-eg-only-quota-raise` | No HPAs; only CPU40/80→64/128, saves exact prior spec |
| `s3-eg-only-create` | Raised reconciled quota; creates only EG min2/max4, CPU200% |
| `s3-eg-only-lower-target` | `--created` exact EG-only create result; fresh integer utilization>=12 and ten-minute Ready EG pair |
| `s3-eg-only-restore-target` | `--created` exact lower result; restores only original CPU target |
| `s3-eg-only-freeze` | `--created` latest confirmed create/lower/restore result; disables both scaling directions |
| `s3-eg-only-delete` | `--created` exact freeze result; UID/RV-guarded removal |
| `s3-eg-only-quota-restore` | `--created` owned quota-raise result; no HPAs, original UID/spec at fixed topology, current generation bound anew, usage below saved ceiling |

Activation rechecks actual/template literal AMOFF/EGON, selected images,
distinct EG hosts, actual/template EGgrace760, PDB, fresh complete EG PodMetrics
and planned process incarnations after approval. AM grace is not raised.
HPA generation is tested when present; UID/freshRV/full-spec CAS is mandatory.
Missing HPA generation is never synthesized; Deployment generation stays mandatory.
Target restore/freeze/delete remain available during topology/metrics failure,
but only for the exact source-bound owned HPA receipt chain. UNKNOWN means
reconcile, never retry. Before later S1 or recovery Deployment writes, freeze
and delete EG HPA, verify both HPAs absent, then use the existing guarded
one-at-a-time descent if needed with actual termination and survivor proof.
Keep down300s/-1 per120s and observe useful automatic up/down, drain and actual
return to two separately; manual descent cannot supply automatic acceptance.

### S1 real production accepted work — binding 19:10 ruling

S1 observes REAL production accepted work through read-only logs/metrics during
an initial 30-minute soak. There is no synthetic write executor; broker creates
and sends remain stopped and the NyxID slot stays untouched. The earlier
synthetic-executor/private-binding HOLD is removed. Bind actual source/images,
serving replicas, baseline and S–E to the existing operator observation path.

Log-engineer01 owns the queries: per-replica create/send/stop/stream success and
latency; ownership and epoch transitions; fencing refusals; independent ledger
duplicate effects (the same logical command executed twice must equal zero);
and 5xx/429 by cause versus baseline. Retain actual accepted-command N and
available per-route counts, failures, uncertainty and terminal/effect outcomes.
Reconciled failures remain failures; balanced accounting alone is not PASS.

Prod-verifier02 adds synthetic READS ONLY to bring critical read routes to
sufficient N: existing Agent open detail/transcript and retained usage 24 h/7 d/30 d,
with separate counts/timings through one owned sampler/lane. Observe existing
lock ownership and pacing; no overlapping collector, synthetic mutation,
retired browser-profile reuse or change to the NyxID reservation.

If accepted production write commands N<20 at 30 minutes, extend the same
observation by up to 30 minutes. If still below 20 afterward, accept on the required
in-code multi-instance suites PASS plus zero errors, disclose actual low N,
and do not hold S1 for low traffic. Record actual extended E and report by E+15.
This sample-count exception does not invent per-route latency or client/render
measurements; missing observations remain explicit. Actual errors, duplicate or
stale effects and observed absolute-target failures remain failures.

Apply A3 and absolute targets to measured boundaries. **Immediately dispatch the
preapproved compatible rollback** for failed in-contract work, accepted-work
loss, duplicate/stale effect, unresolved terminal/effect failure, broken runtime/
auth/provider service or breached absolute latency/recovery/availability targets.
Do not wait for soak end or two five-minute performance windows. Stop promotion,
preserve custody/evidence and use the existing compatible recovery branch.
Record detection, rollback dispatch, restored service and final reconciliation
separately. Retain ownership/receiver/record floors; never blindly resend unknown
work, restart a legacy writer or switch ownership OFF as an automatic recovery.

The [selected S1 runbook](notes/reviews/2026-11-15/s1-execution.md#mandatory-production-accepted-work-observation)
contains the execution/report sequence. Production authority, controller route,
per-stage approvals and 02:00–02:59 blackout remain unchanged. S2 retains its
existing provider-specific accepted-work and terminal reconciliation requirements.

Every stage needs30 stable minutes after its intended replica count is Ready,
healthy in endpoints/NEG, ownership stable, and old processes finished. S1 may
extend this same observation under the low-N rule above. Record
per-minute desired/Ready/Available/terminating, backend health, owner/epoch/stale
effect counters, stream gaps, oldest accepted-unfinished age, terminal/receipt
reconciliation, CPU/throttling/memory/OOM, Mongo occupancy/checkout/timeouts,
worker lag/completion, provider limits/Unknowns and HPA conditions/recommendations.
Include at least two five-minute worker/metering cycles and every route comparison.
Restart stable-soak timing on any image/config/replica change; retain fault/ramp
windows in the results. Correctly rejected stale requests are not committed
stale effects, but sustained refusals still fail availability.

Immediate ABORT: wrong owner/stale committed effect, duplicate external action,
accepted work/terminal lost, two desktop owners, auth bypass, epoch/cursor damage,
zero healthy backends outside the approved cutover, >30s sudden-loss service gap,
no restored service by120s inside the approved cutover, incompatible receiver,
OOM/repeated crash, breached process/provider bounds or Atlas headroom<30%.
Performance-only regression beyond preregistered noise limits aborts after
two consecutive five-minute windows; widespread failures and S1 failed work or
absolute-target violations require immediate compatible rollback. Missing
evidence remains explicit; S1 low accepted-command N follows the extension and
suites-plus-zero-errors acceptance above and does not hold promotion by itself.
Other insufficient evidence retains the applicable stage gate. Unknown/Partial stops promotion
and pages the lead; reconcile within the approved observation window or invoke
compatible recovery. Continue assigned24h post-go-live monitoring with these
triggers even beyond15Nov13:00.

**S3 rollback uses only the linked guarded freeze/delete/descent operations.**
Freeze both directions of each exact stage-created HPA, then remove only those
UID-bound objects using the saved creation results. Verify HPA absence and
stable desired counts before any Deployment descent; an in-flight controller
write may settle late. Each operation needs its own plan and approval.

Keep a safe serving count for current demand. Descend one at a time through
`s3-am-descend` / `s3-eg-descend`, with fresh identity/spec guards, survivor
capacity and Ready/Available/observed-generation checks. Wait for every departing
process to finish and verify survivor traffic/ownership before the next
separately approved decrement. Rollout status alone does not prove termination.

Stop at fixed two with S1/S2 switches/images/PDBs retained, or retain a higher safe
count if demand requires it. Observe30 minutes of recovered performance. For S1
or S2 failure at two, use the corresponding guarded healthy recovery branch
(`s1-one` / `s2-one`) only after HPA absence, actual drain and survivor capacity
are proven; PDB minAvailable1 remains. Unsafe service uses that stage's approved
quiesce/compatible-recovery branch. Keep AM ownership ON and a tested
protocol-compatible image. EG image recovery is **forward-fix-only** unless an
exact older digest has explicit retained-record compatibility proof/approval.

Never use generic `rollout undo`, flip modes OFF, erase epochs, replace user homes
or restore a captured legacy image automatically. AM mode-off needs a separate
full stop, exact-token runtime legacy resets with watermark preservation, proof
for every receiver, then exactly one compatible legacy writer. EG mode-off needs
its own all-writer/retained-record proof. If unsafe, quiesce new admission under
approved controls, preserve fenced custody and ask the lead for recovery.

Receipt per stage: exact source/manifest SHAs, image digests, config/switch digest,
approval IDs/route, time windows, schema/receiver image and exact-duration floor,
retained-runtime inventory hash and positive/negative challenge test artifacts,
all comparisons and failed/NOT_RUN/INCONCLUSIVE cells, resource/HPA/PDB snapshots,
peak live clients/processes, hashes of operator-owned canary receipts, rollback
outcome and remaining gates. Retain sanitized evidence under the engineer's owned
path; remove temporary download/build/dry-run files. Issue closure for #1956
covers delivery of manifests/runbook only; production verification stays on
#1356/#1541/#1530 and the lead's S1/S2/S3 receipts.

## Automatic title provider custody

Before enabling replica mode, every API, title worker and recovery process must
include the title effect custody reader/writer floor. The integrator wires
`MongoSandboxStore::with_title_effect_custody` to the existing replica-mode
configuration. The store default remains OFF. Exclude older title writers before
enabling: they do not enforce the retained barrier and cannot be used for rollback.

A claim first records `title_effect_floor: 1` and a bounded
`title_effect_custody` envelope in its Agent document. Before the provider call,
the floor advances irreversibly to `2` and the envelope freezes the original
claim, first-turn transcript digest, request digest and strategy. Preparation performs no provider request. Only the original
process's consuming prepared capability can dispatch after exact durable intent
confirmation and live-authority validation. There is no provider lookup or retry
for an unresolved intent. A crash before dispatch may therefore leave zero calls
and an unresolved title; a lost provider response may leave one call and the same
unresolved state. A new claim, strategy, model, fallback or user title cannot
create another attempt for that Agent obligation. A retained legacy attempt has
no provider finality proof either: its first custody claim records a permanent
`legacy_unresolved` barrier at floor `2` instead of dispatching again. Exclude all legacy
in-flight title dispatchers during activation, including mode-OFF workers.

Accepted sanitized results are persisted before title publication. A successor
may publish that exact result under its current claim without contacting the
provider. User-title precedence and expiry still fence mutations. An uncertain
intent or result acknowledgement reconciles only that exact stored identity;
absence or a conflicting record never proves permission to repeat a call.

The floor and envelope survive flag OFF, ordinary Agent replacement fields,
log appends, retained reopen, fallback maintenance and user-title writes. Missing,
null after floor `2`, malformed or unknown-version custody fails closed. No TTL or
compaction removes it. The writer inventory uses `$set` for Agent replacements;
title claim/completion writers preserve the separate custody fields. Physical
Sandbox purge refuses unresolved or malformed custody. Purging an accepted result
also requires permanent creation provenance, which survives purge and prevents
recreating the same Agent identity through admission or low-level creation.
Restore Agents, logs and permanent creation provenance coherently. Clearing the
floor or deleting provenance to regain title retries is not a supported recovery.

Synthetic counting-provider tests establish the no-resend invariant. They do
not reconcile real provider outcomes or authorize activation. Retained Unknown
outcomes require a separately qualified provider/finality recovery procedure.
