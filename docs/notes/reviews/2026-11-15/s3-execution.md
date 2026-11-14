# S3 — automatic scaling, 15 November approximately 05:30–06:00 SGT

Use [common setup/safety/acceptance](deployment-runbooks.md). Require signed
S1/S2 PASS; both fixed2, zero terminating processes/clients, real backends and
PDBs healthy; final images/modeON/floors unchanged. No concurrent rollout.
K12–K15/K25 require actual local scale/kill, secure drain, ≤30s same-work recovery,
complete handoff p95, separate hard effective lifetime G, all-process/per-server
capacity with≥30% Mongo headroom, provider/node/quota budgets and calibrated
useful CPU response. Configured grace760 does not prove G or measured p95.

The [revised timeline](stage-timeline.md) orders CPU quota 64/128, HPA and the
separately granted S3b exercise from merged #2038. The reported 20:45 amendment
places Batch10 API-only rolling at AM2/PDB before Activity/U work, targeting 03:00.
For S3, bind observed post-B10 API and B9 EG source/image/replica lifetimes from
engineer02's actual cohort packet. Keep baseline/post cohorts matched, separate
rollout transitions and retain unknown coverage; no planned rollout timestamp
supplies an actual boundary. Bind the final merged controller retaining #2043;
source GO and merge do not supply a production execution grant. Complete required drain/observation/soak before reporting S3
success. If early U2 missed 04:30, engineer02 starts it only after that actual
success and a fresh grant. Failures invoke approved rollback, reporting and lead
re-slotting. U3 must remain at least 15 minutes from the confirmed S3 start.

Keep min2/max5, AM60% of4CPU, EG200% of250m. Up: stabilization0,
Max(+2pods,+100%)/60s. Down: stabilization 300s, **Min(−1 Pod)/120s**.
The lead fixed this cadence: measured handoff p95 must remain ≤90s; otherwise
HOLD and report the incompatibility without changing the interval. Both staged
Deployment templates and actual Pods must have **terminationGracePeriodSeconds:
760** before S3. The 30s preStop delay is included in that grace period. Live
Batch 8 was still 735s at the 14 November readback; S1/S2 guarded template
patches establish 760s. Grace is not proof of a hard socket/client lifetime.
Without rollout, capacity must cover 5+ceil(G/120) processes per service plus all
other consumers; also qualify per-server pool/monitor sockets and fragmentation.
Illustrative G760/P120→12/service is not a qualified budget. Each role's
`release.json` HPA inputs must come from actual qualification.

## Read-only preflight

The metrics-server observation from14Nov16:40 (AM509m, EG285m, bothN1, noHPA)
does not qualify the final candidate or scale-up. Refresh at the assigned window:

```bash
python3 -B "$scale_tools/observe.py" check --service am --count 2 --image "$scale_am_image"
python3 -B "$scale_tools/observe.py" check --service eg --count 2 --image "$scale_eg_image"
k get apiservice v1beta1.metrics.k8s.io \
  -o 'jsonpath={range .status.conditions[*]}{.type}{"="}{.status}{"\n"}{end}'
k get --raw '/apis/metrics.k8s.io/v1beta1/namespaces/chrono-bot/pods?labelSelector=app.kubernetes.io%2Fname%3Dcma-agent-manager'
k get --raw '/apis/metrics.k8s.io/v1beta1/namespaces/chrono-bot/pods?labelSelector=app.kubernetes.io%2Fname%3Dcma-event-gateway'
k get deployment cma-agent-manager cma-event-gateway \
  -o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{range .spec.template.spec.containers[*]}{.name}{" "}{.resources}{"\n"}{end}{end}'
```

Verify PodMetrics timestamp/window freshness and actual requests, not only API
availability. AM target is2.4cores/pod; EG500m/pod. Missing metrics, uncalibrated
useful response or insufficient capacity is HOLD for that qualification. The
lead-selected [S3b scale/drain exercise](s3b-execution.md) temporarily lowers
one target through guarded controller operations; it does not establish
useful-load or max-five performance qualification. Use the existing owner observer for periodic
metrics/Events/currentMetrics/desired/Ready/terminating capture.

## S3 pre-step — raise CPU quota after lead approval

The lead selected raising `ResourceQuota/sandbox-quota` in `chrono-bot`:
**requests.cpu 40 → 64 cores; limits.cpu 80 → 128 cores**. This changes only
admission ceilings, not requests/limits on running workloads. Memory and Pod
quota remain unchanged. Execute before either HPA creation, after S1/S2 PASS
and at the S3 approval boundary. The preparation instruction is not the write
approval; use the exact single-use grant supplied by the lead at S3.

Both services must be fixed at two healthy Pods with no terminating Pods, no
HPAs, and actual/template grace 760s. The existing controller checks these at
plan and again before the guarded write. It also requires quota `status.hard`
to have reconciled to `spec.hard`. Current service, capacity and rollback
preconditions from the common runbook still apply.

```bash
k get resourcequota sandbox-quota -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
k get deployment cma-agent-manager cma-event-gateway \
  -o 'jsonpath={range .items[*]}{.metadata.name}{" grace="}{.spec.template.spec.terminationGracePeriodSeconds}{"\n"}{end}'
k get pods -l 'app.kubernetes.io/name in (cma-agent-manager,cma-event-gateway)' \
  -o 'jsonpath={range .items[*]}{.metadata.name}{" grace="}{.spec.terminationGracePeriodSeconds}{" deleting="}{.metadata.deletionTimestamp}{"\n"}{end}'
plan_scale s3-quota-raise s3-quota-raise
```

The saved `s3-quota-raise.plan.json` contains the complete **before-spec** in
`previous_fields[0].value`; this is quota configuration, without credentials.
Keep the plan before requesting the grant. Planning performs a real server
JSON-patch dry-run. It requires the observed CPU ceilings to still be 40/80 and
prints the exact approval context. The mutation consists of UID, resourceVersion
and full-spec tests followed by only two leaf replacements:
`/spec/hard/requests.cpu` and `/spec/hard/limits.cpu`. No full-manifest apply.

Only after the S3 grant:

```bash
apply_scale s3-quota-raise "${s3_quota_raise_approval:?exact S3 CPU-quota grant}"
k get resourcequota sandbox-quota -o 'jsonpath={.spec.hard}{"\n"}{.status.hard}{"\n"}{.status.used}{"\n"}'
```

Read back 64/128 in **both spec.hard and status.hard** before creating either
HPA. The HPA controller checkpoint refuses stale status or the old ceiling.
The confirmed raise result also saves `quota_before_spec` for restoration.
A conflict is HOLD; a timeout/unknown write must be reconciled against this
exact object before proceeding or replanning. Never retry an ambiguous write.

At 12 AM + 12 EG, the observed other usage gives 51.025 CPU requests / 120.25
CPU limits, within the proposed 64/128 ceilings. That does not add node capacity:
current eligible nodes total 47.67 CPU. Retain the actual placement, memory,
termination/socket and other-consumer checks. The one-per-120s cadence limits
removal rate; observe actual Pod/client exit and do not assume grace alone proves
all termination bounds. No simultaneous rollout is permitted during S3.

### Quota rollback — restore saved CPU ceilings

For an aborted S3, first stop owned load, freeze/delete the exact created HPAs
and finish the serial recovery below. Restore only once both services are fixed
two, actual terminations are finished, HPAs are absent, quota status is current,
and usage is **≤40 requested CPU and ≤80 limit CPU**. If these conditions fail,
retain the raised ceilings while recovering workload; do not lower the quota
and impede replacement admission. Lowering quota does not evict running Pods.

Use the confirmed raise result, not a new baseline or a guessed full spec.
Restoration preserves every unrelated field and refuses a replaced quota or
intervening spec change. It also needs its own fresh exact rollback grant.

```bash
plan_scale s3-quota-restore s3-quota-restore \
  --created "$scale_work/s3-quota-raise.plan.result.json"
apply_scale s3-quota-restore "${s3_quota_restore_approval:?exact quota rollback grant}"
k get resourcequota sandbox-quota -o 'jsonpath={.spec.hard}{"\n"}{.status.hard}{"\n"}{.status.used}{"\n"}'
```

Require the exact saved 40/80 CPU values in spec and reconciled status. If S3
continues successfully, retain 64/128; do not restore quota automatically.

Preparation validation, 14 November 18:19 SGT: the exact UID/RV/spec-guarded
40/80→64/128 patch passed server dry-run against `sandbox-quota`; final readback
remained 40/80. The before-spec and sanitized result are retained locally in
`.github/.cmaeg-local/scale-quota-20261114/`. The actual rollback is prepared
and tested offline; no quota increase or rollback was executed. Controller
suite: 43 tests PASS, including quota identity/drift, CPU-only scope, saved-spec
restoration, rollback usage, fixed cadence and actual 760s Pod grace.

## Create AM HPA, then EG HPA

Each plan performs server dry-run and both-pair checkpoint checks. Get the exact
context's single-use grant before each apply. Preserve creation results; only
these UIDs may be frozen/deleted through this recovery route.

```bash
plan_scale s3-am-create s3-am-create
apply_scale s3-am-create "${s3_am_create_approval:?exact AM HPA-create grant}"
k get hpa cma-agent-manager \
  -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
```

Require actual HPA observed generation, AbleToScale/ScalingActive, fresh current
metrics and idle pair2; explain ScalingLimited. If AM changes before EG create,
hold and re-evaluate; never race an automatic scale with a Deployment patch.

```bash
plan_scale s3-eg-create s3-eg-create
apply_scale s3-eg-create "${s3_eg_create_approval:?exact EG HPA-create grant}"
k get hpa cma-agent-manager cma-event-gateway \
  -o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{.spec}{"\n"}{.status}{"\n"}{end}'
```

If either creation fails/has UNKNOWN outcome, reconcile the named object first.
If only one HPA exists, recover only the actual created UID; never invent a
creation result or delete an object owned by another operation.

## S3b — existing-traffic scale and drain exercise

Lead 14 November 19:33 selected debugger04's sequential AM-then-EG exercise.
Follow [the S3b operator commands](s3b-execution.md) after HPA creation and the
stable-pair baseline. Each temporary target and exact restore uses the merged
controller with a separate operation grant and UID/RV/generation/full-spec tests;
only the CPU target leaf changes. No synthetic writes or direct replica patches.
Observe real third-Pod traffic, restore the original target, wait for actual
automatic return and zero terminations, then ten minutes stable before EG.
Keep down300 -1/120 and actual/template grace760. Missing traffic/drain joins
remain unresolved. The draft's raw patch commands are superseded by this route.

The separate useful-load qualification below remains scoped to its own profile
and lead grant; S3b does not authorize it or turn its unqualified results into PASS.

## Separately qualified useful load and actual automatic return

Load owner: debugger-04/developer-17, with developer-16 metrics calibration.
Developer-16 reports8 network-free safety regressions PASS after debugger-04's
dispatch/selector/observation corrections, including failed-GET/final-observer
abort, cancellation, continuous120s hold and zero terminating on return to2.
The existing8RPS GET profile remains **unqualified** until the owner demonstrates
useful target crossing on the final candidate. Its bounded CLI is below; do not run execution
merely because the local plan prints. No new tooling review is required, but
passing code/tests, calibrated scope and the distinct lead load grant are.

Local plan only (no network):

```bash
scale_s3_driver=/Users/shining/code/work/tech-lead.staff/debugger-04/s3-autoscale-load/s3_autoscale_load.py
python3 -B "$scale_s3_driver" --origin https://cma.nyxid.ai --total-rps 8 \
  --output "$scale_work/s3-load-plan.json"
```

If that exact profile is qualified and authorized, the load owner runs in a
dedicated foreground pane. Credential/fixture environment comes through its
existing private handoff; never echo it, enable shell tracing or read a Secret.
The read-harness grant during soaks is not the S3 stress grant.

```bash
: "${S3_LOAD_APPROVAL_ID:?separate lead load approval}"
: "${NYXID_ACCESS_TOKEN:?existing private owner credential handoff}"
: "${CMA_SYNTHETIC_AGENT_IDS:?approved existing fixture identifiers}"
: "${CMA_SYNTHETIC_CHANNEL_IDS:?approved existing provider fixture identifiers}"
export S3_LOAD_APPROVAL_ID NYXID_ACCESS_TOKEN CMA_SYNTHETIC_AGENT_IDS CMA_SYNTHETIC_CHANNEL_IDS
export S3_KUBE_CONTEXT="$scale_context" S3_KUBE_NAMESPACE=chrono-bot
python3 -B "$scale_s3_driver" --origin https://cma.nyxid.ai --total-rps 8 \
  --execute --ack-production-load --output "$scale_work/s3-cycle.json"
```

Do not replace this with a larger workload without its separately qualified
profile/grant. Record offered/completed useful work, every failed/incomplete
request, actual HPA recommendation/up/down, Ready/service/ownership transitions,
peak clients/processes and full downscale exits. Claim an exercised count only
if observed. Configured max5 is not observed5; manual scale is not K25 proof.
Empty policy/zero samples are INCONCLUSIVE, never PASS. The driver does not
provide a qualified confidence upper-bound method; its cycle result cannot
substitute for the separate performance assessment.

After removing only owned stress, keep the separately owned cooldown canary.
At fixed P120,300s stabilization+three120s descents already takes11minutes
before scheduling/metric/client tails; the driver's12-minute cap is not a
guaranteed successful bound. Timeout/incomplete return is a reported failure,
not permission to shorten cadence or manually manufacture automatic success.
The load owner stops its foreground invocation with Ctrl-C on abort; confirm
its executor exited and in-flight requests settled. Do not kill unrelated work.

Once both actually return2, with no terminating process/client and canonical
service/membership stable, declare S/E for a fresh30-minute final soak. Use the
common acceptance policy and preserve transition failures separately. The
actual stage packet must include replaced/restarted/terminating identities.
M8 verification09:00–16:00 uses prod-verifier01's kit; final report is due18:00.
Provider24-hour monitoring remains separate.

## Recovery — freeze, delete exact HPAs, descend serially

On any common abort trigger, halt only owned load and stop promotion. Freeze
**both directions on both installed HPAs before either deletion**. This does not
cancel an in-flight controller write; read desired/current/actual processes.
Every recovery plan also needs its exact fresh grant. If a write is uncertain,
reconcile first; do not repeat a create/freeze/delete based on timeout alone.

```bash
plan_scale s3-am-freeze s3-am-freeze --created "$scale_work/s3-am-create.plan.result.json"
apply_scale s3-am-freeze "${s3_am_freeze_approval:?exact AM freeze grant}"
plan_scale s3-eg-freeze s3-eg-freeze --created "$scale_work/s3-eg-create.plan.result.json"
apply_scale s3-eg-freeze "${s3_eg_freeze_approval:?exact EG freeze grant}"
k get hpa cma-agent-manager cma-event-gateway \
  -o 'jsonpath={range .items[*]}{.metadata.name}{" "}{.spec.behavior}{"\n"}{end}'
plan_scale s3-am-delete s3-am-delete --created "$scale_work/s3-am-create.plan.result.json"
apply_scale s3-am-delete "${s3_am_delete_approval:?exact AM UID-delete grant}"
plan_scale s3-eg-delete s3-eg-delete --created "$scale_work/s3-eg-create.plan.result.json"
apply_scale s3-eg-delete "${s3_eg_delete_approval:?exact EG UID-delete grant}"
k get hpa cma-agent-manager cma-event-gateway --ignore-not-found -o name
```

The last command must return no objects. Server DELETE dry-run uses UID/RV
preconditions; a same-name replacement is refused. For partial creation, omit
the never-created service's steps, retaining the actual other creation result.
Both HPAs must be absent before any Deployment mutation.

For each role, wait all previous terminations, fully healthy survivors and
fresh≤120s capacity assessment. If demand needs>2, retain the safe fixed count
and report autoscaling not delivered. Choose a new label for each one-step
descent; no loop/bulk5→2 is allowed. Exact command for an AM5→4 checkpoint:

```bash
python3 -B "$scale_tools/observe.py" check --service am --count 5 --image "$scale_am_image"
plan_scale s3-am-descend s3-am-5-to-4
apply_scale s3-am-5-to-4 "${s3_am_descent_approval:?exact fresh one-owner descent grant}"
python3 -B "$scale_tools/observe.py" check --service am --count 4 --image "$scale_am_image"
```

After actual process/client exit, survivor service and refreshed capacity, run
the same action with a **new** label `s3-am-4-to-3`, then `s3-am-3-to-2`, each with
its own plan/grant/check at actual counts. For EG use `s3-eg-descend`, labels
`s3-eg-5-to-4` / `s3-eg-4-to-3` / `s3-eg-3-to-2`, and `--service eg --image "$scale_eg_image"`.
Skip counts never reached; the generator reads current count and decrements
exactly1, refusing≤2. If a new plan's actual transition differs from the intended
label, stop and reconcile before granting it. Unhealthy owners cannot use this
healthy-descent path: keep compatible fences and obtain the separately approved
all-stop recovery branch after HPAs are absent.

Preserve PDBs, modeON, compatible images and custody. Verify both fixed2 and real
service, then fresh30-minute recovery soak. Fixed replicas are not autoscaling
PASS. A rollback of recovery is never implicit HPA recreation; restarting S3
requires a fresh explicit lead decision and new operation approvals.

## Admission and termination-capacity checkpoint

[Production server dry-run results](server-dry-run-20261114.md) accepted both HPAs
and their 60%/200% targets, min 2/max 5 and structural P120 cadence. Metrics were
fresh; neither HPA was created. Steady AM5/EG5 fits the observed namespace quota
and a 2/2/1 placement across three eligible shared hosts. This does not establish
capacity during termination: 8 AM + 8 EG already requires 80.25 CPU limits versus 80;
the illustrative 12+12 envelope requires 51.025 CPU requests and 120.25 CPU
limits. The lead-selected pre-step raises those ceilings to 64/128, subject to
the S3 grant and readback. Twelve AM alone still exceed all 47.67 eligible-node CPU.

Before either HPA creation, require measured hard lifetime/cadence and a refreshed
combined process budget that passes namespace quota, per-node placement, actual
memory and all existing consumer/socket limits. If this does not fit, retain
fixed two and escalate the specific capacity remedy for lead approval;
never treat maxReplicas 5 as a total live-process cap. Repeated dry-runs do not
reserve quota. This capacity finding does not authorize target/schedule changes;
the only selected temporary target exception is the separately approved S3b leg.
