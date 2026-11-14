# S3 — EG-only autoscaling after S2, before S1

Executor: deployment-engineer-01. Lead grants each operation separately.
This route implements the 15 November 03:35 architecture ruling and 03:50/05:18
Track A assignments. Use the controller reviewed in #2065 or its verified merged
successor. It permits healthy **AM1/OFF + EG2/ON after S2 PASS**; it does not require
S1 PASS, AM2 or an AM grace760 rollout. It does not qualify paired S3b.

Use [common setup, single-writer safety and acceptance](deployment-runbooks.md)
and [the packet template](../../../../.github/scripts/cma_scale/eg-only.release.example.json).
The [paired S3 route](s3-execution.md) remains separate. Neither a review verdict,
a merge, a local mechanics PASS nor these commands is a production grant.
Reserve one release writer across both Deployments/HPAs; no rollout, manual scale
or competing operator overlaps this sequence. Keep the common write blackout.

## Bind actual evidence before planning

Require a completed, accepted **30-minute S2** at the selected immutable sources
and images, including console shared-session behavior, replica storage/schema,
provider/session authority, accepted-work reconciliation and compatible ON
recovery. Missing S2 PASS is HOLD. Local #2068/old5cfd readiness exercises and
N1 CPU metrics do not substitute for production S2 or fresh N2 qualification.

AM remains one healthy serving Pod with literal replica mode OFF in its actual
Pod and template, selected image and exact Deployment/process incarnation.
Preserve its actual template and grace. EG starts at two healthy serving Pods on
distinct hosts, literal ON in actual Pods/template, selected image, **actual and
template grace760**, 250m CPU request and current minAvailable1 PDB. Both HPAs
must be absent before quota mutation/initial creation. No terminating or foreign
selected Pods, stale Deployment status, AM HPA or incomplete identity is allowed.

After common operator setup defines `scale_tools`, `scale_work`, `plan_scale`
and `apply_scale`, select a separate private EG-only input file:

```bash
scale_release="$scale_work/eg-only-release.json"
test ! -e "$scale_release"
cp "$scale_tools/eg-only.release.example.json" "$scale_release"
```

Populate only actual observations; nulls/false defaults intentionally refuse.
`source` is the full application source SHA, not the controller commit or image
digest. Bind both selected immutable images. `eg_only_hpa.s2` needs state PASS,
matching source/images, actual started/completed UTC times, evidence SHA256 and
each exact Deployment binding (`uid_sha256`, `generation`, `spec_sha256`).
Keep the private source evidence accessible to the executor and reviewer; a
boolean or hash alone is not the underlying qualification.

`hpa.eg` retains qualified handoff p95<=90s, CPU signal qualification and the
separate complete hard lifetime **G>=760s**. Capacity includes at least
**5+ceil(G/120)** live-plus-terminating EG processes (**12 at G760**), AM and all
other consumers, per-server pools/monitor sockets and fragmentation, >=30% Mongo
headroom, actual node/Pod placement, memory/CPU/quota, provider reconnect budgets,
survivor service and early secure drain. Native preStop sleep is propagation,
not proof of early drain. `eg_only_hpa.capacity` records those qualifications,
its evidence SHA256 and an observation <=120s old at plan and apply. Refresh
expired capacity evidence and replan; never edit a previously approved plan.

The production HPA remains **min2/max5, CPU200%**, up0s Max(+2Pods,+100%)/60s,
down300s Min(-1Pod)/120s. Do not copy the local max3 cap, U4/target3 input,
resources or emulation latency into this packet. Source-bound p95, useful CPU
response, complete hard lifetime and capacity remain production inputs.

## Read-only preflight

Run at the actual operation boundary; existing owners retain their observers.
These projections do not read Secret payloads or start another watcher.

```bash
python3 -B "$scale_tools/observe.py" check --service am --count 1 --image "$scale_am_image"
python3 -B "$scale_tools/observe.py" check --service eg --count 2 --image "$scale_eg_image"
k get hpa cma-agent-manager cma-event-gateway --ignore-not-found -o name
k get pdb cma-event-gateway -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
k get resourcequota sandbox-quota -o 'jsonpath={.spec.hard}{"\n"}{.status.hard}{"\n"}{.status.used}{"\n"}'
k get --raw '/apis/metrics.k8s.io/v1beta1/namespaces/chrono-bot/pods?labelSelector=app.kubernetes.io%2Fname%3Dcma-event-gateway'
```

Require complete PodMetrics for every actual container, timestamp age and sample
window <=60s, actual requests and a qualified CPU signal. Availability of the
metrics API alone is insufficient. Planning and postapproval checks additionally
bind modes, selected runtime images, Pod/process incarnations, PDB and topology.

## Quota and EG HPA creation

If the qualified live quota is still40/80 CPU, plan the reviewed two-leaf raise.
If it is already64/128, require matching reconciled status and qualified usage;
do not fabricate an owned raise receipt or rerun a40/80-only transformation.
Memory, Pod quota and workload resources are preserved.

```bash
plan_scale s3-eg-only-quota-raise s3-eg-only-quota-raise
```

After the lead grants that printed exact context:

```bash
apply_scale s3-eg-only-quota-raise "${s3_eg_quota_raise_approval:?exact EG-only quota grant}"
k get resourcequota sandbox-quota -o 'jsonpath={.spec.hard}{"\n"}{.status.hard}{"\n"}'
```

With64/128 in both spec/status, create only EG HPA. Never run `s3-am-create`.

```bash
plan_scale s3-eg-only-create s3-eg-only-create
```

After its separate exact-context grant:

```bash
apply_scale s3-eg-only-create "${s3_eg_create_approval:?exact EG-only HPA-create grant}"
scale_eg_state_result="$scale_work/s3-eg-only-create.plan.result.json"
k get hpa cma-event-gateway -o 'jsonpath={.metadata.uid}{"\n"}{.spec}{"\n"}{.status}{"\n"}'
```

Keep the actual create UID/result and strict admission/readback equality.
HPA generation/observedGeneration may be absent: never manufacture them; supplied
invalid/stale values refuse. Every patch still tests actual UID, freshRV and
full spec, with generation when present. Deployment generation remains required.
Before exercise, require current AbleToScale/ScalingActive, explain ScalingLimited,
fresh HPA utilization and complete PodMetrics. If creation is UNKNOWN, reconcile
that exact object before any further operation; no automatic retry or raw apply.

## One EG target exercise and actual automatic return

Keep AM1/OFF fixed and the original EG HPA policy. Lowering requires the EG pair
Ready for ten minutes, actual integer HPA utilization **U>=12**, and
**T=max(1,floor(5U/6))** strictly below the original200. Qualify the installed HPA
tolerance against U/T-1. Missing/low/stale metrics HOLD the exercise; no request
budget increase or production load follows from this packet.

```bash
plan_scale s3-eg-only-lower-target s3-eg-only-lower-target \
  --created "$scale_work/s3-eg-only-create.plan.result.json"
```

After its separate exact-context grant:

```bash
apply_scale s3-eg-only-lower-target "${s3_eg_lower_approval:?exact EG-only target-lower grant}"
scale_eg_state_result="$scale_work/s3-eg-only-lower-target.plan.result.json"
```

Only the CPU-target leaf changes. Observe actual automatic growth, new Pod
UID/IP/owner, selected image, Ready/backend eligibility and real successful
traffic on the new Pod for the existing five-minute witness. Preserve every
additional replica if the max5 policy grows beyond3; never report a2→3→2
trajectory if the observed path differs. Preserve complete request/accepted-work
counts, errors and lifecycle joins; missing traffic/drain remains unresolved.

Plan exact target restoration from the confirmed lower result, including on an
incident. Health or metrics failure must not prevent this guarded recovery.

```bash
plan_scale s3-eg-only-restore-target s3-eg-only-restore-target \
  --created "$scale_work/s3-eg-only-lower-target.plan.result.json"
```

After its own exact-context grant:

```bash
apply_scale s3-eg-only-restore-target "${s3_eg_restore_approval:?exact EG-only target-restore grant}"
scale_eg_state_result="$scale_work/s3-eg-only-restore-target.plan.result.json"
```

Read back the original200 target and complete original spec. Keep observation
through actual automatic return2, zero terminating processes/clients and healthy
survivors. The300s stabilization is a recommendation-history window, not a new
300s timer from restoration. Record HPA scale timestamps and observed boundaries
separately; don't infer raw recommendation history or drain start from Pod
absence/deletion deadline. Keep downsteps no faster than the qualified120s policy.

Start the common30-minute stable acceptance window only after actual return and
termination completion. Bind a fresh60-minute before window to the actual stage
start, matched backfill state and selected source/images; use existing read/log
owners, preserve actual S/E and report by E+15. Readiness GET continuity cannot
prove accepted work was in flight at application drain start. Require actual
lifecycle/terminal/effect reconciliation for that claim; no local fallback or
S1 low-N exception is automatically available to this EG stage.

## Incident recovery and later S1 non-overlap

Use the common immediate safety/availability/absolute-performance abort rules
and compatible ON recovery. UNKNOWN writes require reconciliation, never replay.
The exact owned target can be restored during telemetry failure. Freeze/delete
also use owned source-bound receipts and do not require a healthy topology.

For an aborted leg or before later S1/recovery Deployment writes, select the
latest **confirmed** EG-only create/lower/restore result in `scale_eg_state_result`.
Never use an UNKNOWN result or invent a receipt. Plan/approve freeze, then
plan/approve deletion from the new freeze result:

```bash
plan_scale s3-eg-only-freeze s3-eg-only-freeze --created "$scale_eg_state_result"
```

```bash
apply_scale s3-eg-only-freeze "${s3_eg_freeze_approval:?exact owned EG HPA-freeze grant}"
plan_scale s3-eg-only-delete s3-eg-only-delete \
  --created "$scale_work/s3-eg-only-freeze.plan.result.json"
```

```bash
apply_scale s3-eg-only-delete "${s3_eg_delete_approval:?exact owned EG HPA-delete grant}"
k get hpa cma-agent-manager cma-event-gateway --ignore-not-found -o name
```

Verify no HPA objects remain. Every Deployment mutation still requires both
HPAs absent; this route supplies no bypass. If EG remains above2 after an abort,
use the existing guarded `s3-eg-descend` one Pod at a time, with new labels/grants,
actual previous termination and fresh survivor-capacity evidence. Manual descent
is recovery, not automatic-scaling acceptance. Do not mutate AM for this leg.

Only if this packet owns a confirmed quota raise, both HPAs are absent, the
original fixed AM1/EG2 UID/full specs have returned, capacity is fresh and used
CPU is<=saved40/80 may its quota be restored. A completed HPA cycle may advance
historical Deployment generation; the new restoration plan binds current
generation and refuses any subsequent drift. Keep raised quota while recovery
needs it; quota restoration is not automatic on successful S3.

```bash
plan_scale s3-eg-only-quota-restore s3-eg-only-quota-restore \
  --created "$scale_work/s3-eg-only-quota-raise.plan.result.json"
```

```bash
apply_scale s3-eg-only-quota-restore "${s3_eg_quota_restore_approval:?exact owned EG quota-restore grant}"
```

Report actual controller/application SHAs, immutable images, every plan/result,
source-bound S2 receipt, metric/capacity windows, offered/accepted/terminal/error
counts, growth/retirement joins, automatic versus manual return, unchanged AM,
final target/HPA ownership, quota and cleanup. Keep production acceptance,
local #2068 session/HPA evidence and historical5cfd mechanics as separate rows.
