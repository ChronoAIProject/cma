# S3 — automatic scaling, 15 November 10:30 SGT

Use [common setup/safety/acceptance](deployment-runbooks.md). Require signed
S1/S2 PASS; both fixed2, zero terminating processes/clients, real backends and
PDBs healthy; final images/modeON/floors unchanged. No concurrent rollout.
K12–K15/K25 require actual local scale/kill, secure drain, ≤30s same-work recovery,
complete handoff p95, separate hard effective lifetime G, all-process/per-server
capacity with≥30% Mongo headroom, provider/node/quota budgets and calibrated
useful CPU response. Configured grace760 does not prove G or measured p95.

Keep min2/max5, AM60% of4CPU, EG200% of250m. Up: stabilization0,
Max(+2pods,+100%)/60s. Down: stabilization300s, Min(−1pod)/P;
P120 if p95≤90, otherwise ceil(p95+30), at most1800, never clamped.
Without rollout, capacity must cover5+ceil(G/P) processes per service plus all
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
useful response or insufficient capacity is HOLD. Do not lower targets merely
to manufacture a scale event. Use the existing owner observer for periodic
metrics/Events/currentMetrics/desired/Ready/terminating capture.

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

## Owned useful load and actual automatic return

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
At default P120,300s stabilization+three120s descents already takes11minutes
before scheduling/metric/client tails; the driver's12-minute cap is not a
guaranteed successful bound. Timeout/incomplete return is a reported failure,
not permission to shorten cadence or manually manufacture automatic success.
The load owner stops its foreground invocation with Ctrl-C on abort; confirm
its executor exited and in-flight requests settled. Do not kill unrelated work.

Once both actually return2, with no terminating process/client and canonical
service/membership stable, declare S/E for a fresh30-minute final soak. Use the
common acceptance policy and preserve transition failures separately. The
actual stage packet must include replaced/restarted/terminating identities.
M8 verification13:00–17:00 and provider24-hour monitoring remain separate.

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
