# S1 — AM fixed two, 15 November 06:00 SGT

Use [common setup, policy, observations and acceptance](deployment-runbooks.md)
first. Executor deployment-engineer-01; lead owns GO. Preconditions are K01–K25
as applicable, the revised M3 gate and architect04 GO, final M4 results,
compatible retained receiver/schema, survivor
capacity, no HPAs or other rollout writer, EG proven healthy at1, canonical
backend observer, qualified <120s no-overlap variant and both recovery branches.
No final image, timing proof or production approval is supplied by this file.

The binding lead ruling of 14 November 19:10 SGT replaces the earlier
synthetic-write plan. S1 observes real production accepted work through read-only
logs and metrics. There is no synthetic write executor: broker creates/sends
remain stopped and the NyxID slot stays untouched. The former executor/private
binding HOLD is removed. Prod-verifier02 supplies synthetic reads only, as below.

The lead has approved read-only cutover observations. Use the agreed
[S1 observer commands and timing bounds](s1-observation.md). Canonical
Service/EndpointSlice/NEG annotation mapping is observed and the owner cutover
CLI is supplied. Compute access/resource mapping, actual probe inputs/window and
independent replacement-source association remain technical HOLDs. Generic
read-only observation approval need not be requested again.

Bind the actual selected images from the completed local release input:

```bash
scale_am_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["images"]["am"])' "$scale_release")
scale_eg_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["images"]["eg"])' "$scale_release")
python3 -B "$scale_tools/observe.py" check --service am --count 1 --image "$scale_am_image"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 --image "$scale_eg_image"
```

Each pair below is **separate**: plan/server dry-run, obtain that printed context's
single-use approval, apply/readback, then check the stated checkpoint. Never
paste the page as an unattended script. Set approval variables only after the
lead has granted the exact plan. Existing matching PDB may be retained after
its full spec is checked; any other existing PDB is HOLD.

1. Pause legacy1. The original Pod remains serving. Rollback before stop: branch A.

```bash
plan_scale s1-pause s1-pause
apply_scale s1-pause "${s1_pause_approval:?exact pause grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 1 \
  --image "$scale_am_image" --paused-template
```

2. Prepare the inactive modeON template: final image, Pod-IP relay8080, owned
capacity, grace760, readiness/topology and approved AM drain hook. Preserve all
unrelated env/config/resources. Wait for the Deployment's observed generation;
verify the original Pod remains the only process. Rollback: branch A.

```bash
plan_scale s1-prepare s1-prepare
apply_scale s1-prepare "${s1_prepare_approval:?exact inactive-template grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 1 \
  --image "$scale_am_image" --paused-template
```

3. Confirm the independent service observer and recovery operator are ready.
Require successful mapped Compute reads and the bound cutover probe; refresh
the old Pod UID/restart/image cohort after M4. Follow the observer's query
brackets and conservative deadline, not a first-absent timestamp.
Before stop, confirm owner `operator_trigger_ready`, the actual N1/OFF pre-arm
context and available shared sample spacing. Trigger the one pair only after
qualified withdrawal and authority cessation; retain its final result and the
independent replacement-source join, not the trigger acknowledgment.
Stop legacy1→0. This is the last safe boundary for branch A. Observe actual old
backend withdrawal; start the strict clock from that observation. All previous
process, socket and effect authority must end or be safely revoked/expired.
No claim clearing. After this write use branch C for failure.

```bash
plan_scale s1-stop s1-stop
apply_scale s1-stop "${s1_stop_approval:?exact stop and outage grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 0 --image "$scale_am_image"
```

4. Only after actual zero plus independent authority cessation, unpause0.
Wait for the controller-created candidate ReplicaSet with desired/observed0,
all older RS0 and current Deployment observed generation. The next plan verifies
this; a failure is HOLD/recovery within the same clock. No hand-created RS.

```bash
plan_scale s1-unpause s1-unpause
apply_scale s1-unpause "${s1_unpause_approval:?exact unpause-zero grant}"
```

5. Start0→2 through the A2 cutover exception. Observe qualified full user-service
completion with a defensible outage upper bound strictly below120s; also record
when both are Ready/Available, serving through
canonical backends, independent members and on distinct hosts. Readiness alone
does not end the outage. Branch C handles failed start; branch B requires two
healthy owners and fresh survivor capacity.

```bash
plan_scale s1-start s1-start
apply_scale s1-start "${s1_start_approval:?exact start-two grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 2 --image "$scale_am_image"
python3 -B "$scale_tools/observe.py" check --service eg --count 1 --image "$scale_eg_image"
```

6. After qualified two, install AM PDB minAvailable1/AlwaysAllow. Its selector
is exactly `app.kubernetes.io/name=cma-agent-manager`. Retain it in recovery.

```bash
plan_scale s1-pdb s1-pdb
apply_scale s1-pdb "${s1_pdb_approval:?exact PDB-create grant}"
k get pdb cma-agent-manager -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
```

Declare actual soak S/E, update actual source/Pod packets, and follow common
30-minute measurement/acceptance and the mandatory production observation below.
Capture cross-ingress, background work, fences/custody, desktop exclusion and
all applicable route/client targets.
Lead's signed S1 PASS permits S2; no automatic advancement.

## Mandatory production accepted-work observation

Deployment-engineer01 owns execution and immediate compatible recovery.
Log-engineer01 owns the read-only logs/metrics queries over REAL production
traffic. Start the initial 30-minute soak only after the intended replica count,
backends, membership and handoffs are stable. Bind the queries to the actual
source/images/config, serving replica identities, baseline and S–E window.
No synthetic create, Send, Stop or provider workload is authorized by this plan.
The stopped broker mutation path stays stopped and the NyxID slot is untouched.

The [19:56 U/S schedule](stage-timeline.md) keeps S1 at06:00 even when U2
backfill is still active; replica maintenance is lease-fenced. Engineer02 supplies
the actual backfill state/timestamps. Log01 tags latency/error cohorts with that
state and uses an AM1 baseline from the same state. Split observed state changes
instead of pooling active/inactive windows; unknown state or missing matched
baseline stays explicit. Preserve the original accepted-command denominator and
cumulative errors across those cohorts and any low-N extension. A delayed U2
targets06:45 only after this actual S1 soak ends. U1–U3 remain engineer02-owned
and each U/S stage still requires its own fresh lead execution approval.

1. Observe per-replica success, failure and latency for create, send, stop and
   streams. Record actual distinct accepted-command N and available per-route
   counts; retries and synthetic reads do not increase this denominator.
   Keep operation/owner/epoch linkage and client-boundary attribution where
   present; never invent serving-instance identity, frame/render completion or
   latency from transport headers. Unobserved client boundaries remain explicit.
2. Query ownership and epoch transitions, fencing refusals, and the independent
   ledger duplicate-effect check: the same logical command executed twice must
   equal zero. Reconcile available accepted/terminal/effect outcomes and retain
   failed, incomplete, uncertain and stale-effect observations. Correct fencing
   refusals are safety evidence, distinct from a committed stale effect. Compare
   5xx and 429 counts/rates by cause with the matched baseline.
3. Prod-verifier02 runs synthetic READS ONLY through one owned sampler/lane:
   existing Agent open detail/transcript and retained usage 24 h/7 d/30 d. Keep
   separate range/route counts and timings to bring critical read routes to
   sufficient N. Bind the sampler to its actual release/window and respect its
   shared lock and pacing. Never start an overlapping collector, bypass a live
   lock, reuse the stopped mutation path or touch the NyxID reservation.
4. At 30 minutes, if fewer than 20 production write commands were accepted, extend
   the same observation soak by up to 30 minutes and record revised E. If N is
   still below 20 after the extension, **accept on passing required in-code
   multi-instance suites plus zero errors**, disclose actual low N and the
   extension in the report, and do not hold S1 for low traffic. This is a stage
   acceptance exception for sample count; it supplies no invented per-route
   measurement or unobserved client-boundary PASS. Actual failures remain
   failures, including duplicate/stale effects and observed absolute violations.
5. Apply unchanged A3 classes and absolute targets to the boundaries actually
   measured: create≤5s, open/Send/interrupt≤1s, usage≤2s, and the common applicable
   route limits. Preserve all errors, timeouts, refusals, incomplete outcomes and
   baseline/telemetry gaps. Low N alone follows the explicit exception above;
   missing evidence is not fabricated or converted into a measurement.
6. On accepted-work loss, duplicate/stale effects, authorization breach, dual
   desktop authority, unresolved terminal/effect failure, broken runtime/auth/
   provider service, failed in-contract journeys or breached absolute completion,
   recovery or availability targets, **stop promotion and immediately dispatch
   the approved compatible rollback**. Do not wait for soak end or two
   consecutive five-minute performance windows. Engineer01 uses branch B only
   with healthy owners and fresh survivor capacity; otherwise use approved
   branch C, preserving custody and receiver/ownership/record floors. Never
   blindly resend or delete unknown work. Record detection, rollback dispatch,
   restored successful service and final reconciliation as separate times.

Report by actual E+15 minutes: selected source/images/config, S/E and any
extension, query and read-sampler bindings, backfill state and matched baseline
intervals, actual accepted-command N, available
per-replica/route counts, ownership/fencing/ledger results, 5xx/429 causes versus
baseline, read-range timings, all failed/incomplete outcomes, and any recovery.
If the low-N fallback is used, state its required-suite PASS and zero-error
basis alongside the actual N and unobserved boundaries. The former synthetic
write executor requirement is closed; existing production approvals, compatible
rollback and 02:00–02:59 SGT no-write rules remain.

## Recovery A — before old stop, original one still serving

If only pause applied, omit `--prepare-plan`. If preparation also applied,
include it as below. The controller reconstructs only changed fields, verifies
the original full spec and original controlled Pod, and restores the inactive
template/unpaused1. Any drift/new process/post-stop state refuses this branch.

```bash
plan_scale s1-abort-before-stop s1-abort-before-stop \
  --pause-plan "$scale_work/s1-pause.plan.json" \
  --prepare-plan "$scale_work/s1-prepare.plan.json"
apply_scale s1-abort-before-stop "${s1_abort_approval:?exact pre-stop abort grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 1 --image "$scale_am_image"
```

If abort fails/has UNKNOWN outcome, reconcile first; never stop the serving old
owner merely to make the abort command pass. Failed dry-run writes nothing.

## Recovery B — qualified healthy two to compatible modeON one

Refresh `survivor_capacity.am` from the capacity owner's actual observation.
Both owners must be healthy with no existing termination; observation≤120s old,
minimum safe count≤1. The image, modeON, epochs, fences and PDB remain.

```bash
plan_scale s1-one s1-one
apply_scale s1-one "${s1_one_approval:?exact healthy-one recovery grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 1 --image "$scale_am_image"
```

Wait actual process/client exit and survivor service, then fresh30-minute
recovery soak. Downscale never counts as a successful S1 delivery. If unsafe or
unhealthy, use the separately authorized branch C instead.

## Recovery C — failed start/unhealthy owner, compatible all-stop restart

This requires explicit outage/recovery authority before initial cutover. Stop
promotion/admission through the approved controls; preserve accepted custody.
Quiesce to paused0, wait real zero and independent remote-authority cessation.
If safe takeover cannot be established, stay quiesced and escalate. This branch
may exceed120s; report the failed outage budget, never hide it.

```bash
plan_scale s1-quiesce s1-quiesce
apply_scale s1-quiesce "${s1_quiesce_approval:?exact emergency all-stop grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 0 --image "$scale_am_image"
plan_scale s1-recovery-image s1-recovery-image
apply_scale s1-recovery-image "${s1_recovery_image_approval:?qualified compatible image grant}"
plan_scale s1-unpause s1-recovery-unpause
apply_scale s1-recovery-unpause "${s1_recovery_unpause_approval:?exact recovery unpause grant}"
# Wait for actual controller-owned compatible RS0; the next plan enforces it.
plan_scale s1-recover-one s1-recover-one
apply_scale s1-recover-one "${s1_recover_one_approval:?exact compatible restart grant}"
scale_am_recovery_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["recovery"]["am"]["image"])' "$scale_release")
python3 -B "$scale_tools/observe.py" check --service am --count 1 --image "$scale_am_recovery_image"
```

Verify canonical user service, receiver/fence compatibility, custody reconciliation
and actual old-client exit, then fresh30-minute recovery soak. No generic
`rollout undo`, modeOFF, receiver reset or direct Pod/DB patch is part of recovery.
If failure happens during recovery itself, reconcile its last write; select a
new unique plan name and grant only after the actual state is understood.

## Admission and placement checkpoint

[Batch 8 server dry-run results](server-dry-run-20261114.md) passed for AM2, AM PDB
and AM HPA. S1 AM2/EG1 projects 8.275 requested CPU and 18.25 CPU limits against
namespace caps 40/80, with four total namespace Pods in the observed snapshot.
Three shared hosts are eligible; refresh per-host free requests and actual memory
before stop, then verify distinct-host placement after start. AM observed memory
was 3273.7 MiB against 256 MiB requested, so request arithmetic alone is insufficient
for memory headroom. Full-manifest admission also changes probes, metrics and
historical annotations; use the prepared narrow stage operation and actual final
image. Admission success does not establish drain-hook support or outage timing.
