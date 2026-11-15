# S3b — sequential HPA scale and drain exercise

Lead 14 November 19:33 selected debugger04's scale-exercise draft for S3.
Deployment-engineer01 executes through the merged guarded controller, with a
fresh per-operation grant. This preparation is not production approval. Use
the [common setup](deployment-runbooks.md) and [S3 preconditions/recovery](s3-execution.md).
Existing real production traffic supplies the requests; no synthetic writes,
broker creates/sends/stops, NyxID-slot changes or direct Deployment scaling.

Exercise AM first, return it to two Ready Pods with zero terminating processes,
then keep it stable for ten minutes before EG. Never lower both targets at once.
Images, templates, CPU requests, replica bounds and scaling policies stay fixed.
The mutation is one CPU target leaf, followed by its exact guarded restoration.
This demonstrates only the observed HPA transition and drain. It does not prove
useful-load calibration, max-five performance or an unobserved client boundary.

| Service | Original target | Bounds | Downscale |
|---|---:|---|---|
| AM | 60% of 4 CPU | min2 / max5 | 300s stabilization, one Pod/120s |
| EG | 200% of 250m CPU | min2 / max5 | 300s stabilization, one Pod/120s |

## Before either leg

Require the S3 CPU quota raise/readback, complete capacity/termination bounds,
handoff p95≤90s, both actual Pod and template grace760 including preStop, and
the stable original HPA policies. Current historical Batch8 grace735 is not
qualification. Both Deployments must have been at two healthy Ready Pods for
at least ten minutes, with no termination or concurrent rollout. Confirm both
PDBs and actual backend health. Bind the ten-minute request/error/latency
baseline, original Pod UID/IP/container incarnation/restart counts, Deployment
generation/template/image and existing HPA creation results in the stage packet.

The controller requires exactly the original single CPU Resource/Utilization
metric and complete original policy, current AbleToScale/ScalingActive and
observed generation, both healthy pairs, and PodMetrics for every selected
Pod (timestamp age≤60s, sampling window≤60s). Explain ScalingLimited and verify
the installed HPA tolerance; the selected 1.20 ratio must exceed it. The fresh
status utilization must be an integer≥12. Missing/stale/unknown metrics refuse
the leg. The operator baseline and log joins remain required observations.

The target is `max(1, floor(current_utilization / 1.20))`, and must be below
the saved original target. This exceeds the usual 10% tolerance; it does not
guarantee scaling under changing traffic or HPA damping. The controller binds
that observed utilization and rechecks it before dispatch; change requires a
fresh plan, never an automatic retry with weaker guards.

Reserve time for the five-minute scale-up witness, up to25 minutes after target
restoration, the ten-minute gap between legs and the final30-minute stable
soak. Coordinate the declared S3 window with the lead. Do not compress drain
or the 02:00–02:59 blackout to fit the schedule. Prearrange compatible recovery
and the exact guarded restoration grant before lowering either target.

## AM lower, traffic witness and restore

Use the confirmed AM HPA creation result, whose source and UID must match.
`plan_scale` is server dry-run only; `apply_scale` consumes the plan's exact grant.

```bash
plan_scale s3-am-lower-target s3b-am-lower \
  --created "$scale_work/s3-am-create.plan.result.json"
apply_scale s3b-am-lower "${s3b_am_lower_approval:?exact AM target-change grant}"
k get hpa cma-agent-manager -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
```

Retain the lower result and original HPA spec. Each lower/restore write tests
fresh UID, resourceVersion, generation and the entire expected spec, then
replaces only `/spec/metrics/0/resource/target/averageUtilization`. Status churn
can advance resourceVersion: reread it when planning/applying restoration; never
omit its guard. Spec/identity drift refuses restoration and escalates immediately.

Observe HPA desired/current values, each new Pod UID/IP, Ready transition,
endpoint/NEG membership, first successful request with upstream join, and
request status/latency. A third Ready Pod and real successful request witness
must appear within five minutes. Otherwise restore the target and report the
incomplete/failed leg; absent traffic does not prove service or drain success.
Restore immediately after the successful witness or on any abort:

```bash
plan_scale s3-am-restore-target s3b-am-restore \
  --created "$scale_work/s3b-am-lower.plan.result.json"
apply_scale s3b-am-restore "${s3b_am_restore_approval:?exact AM target-restore grant}"
k get hpa cma-agent-manager -o 'jsonpath={.spec}{"\n"}{.status}{"\n"}'
```

Restoration uses the confirmed lower result's saved original spec/target; it
does not recapture a new baseline. Failed metrics/readiness do not block this
guarded target restoration. An UNKNOWN lower write must first be reconciled
against its exact identity/spec before an owner-approved restore is prepared.
No unguarded edit, scale, delete or target overwrite is permitted.

Expect the first desired-replica decrement roughly300–420s after restore if
metrics recommend two. Grace760 may then keep the retiring process alive;
allow up to25 minutes of observation, including additional120s decrements if
four/five were reached. These are planning bounds, not measured completion.
At timeout, retain unresolved evidence and invoke the approved recovery decision;
never declare completion without two Ready Pods, zero terminating Pods/clients,
the original target/policy and successful survivor service. Never shorten grace.

## EG only after AM is restored and stable

After AM's actual return and ten-minute stable interval, capture a new EG
baseline and repeat the same bounded sequence. The controller requires both
original HPA policies before any lower operation.

```bash
plan_scale s3-eg-lower-target s3b-eg-lower \
  --created "$scale_work/s3-eg-create.plan.result.json"
apply_scale s3b-eg-lower "${s3b_eg_lower_approval:?exact EG target-change grant}"
```

After the qualifying new-Pod traffic witness, or immediately on abort:

```bash
plan_scale s3-eg-restore-target s3b-eg-restore \
  --created "$scale_work/s3b-eg-lower.plan.result.json"
apply_scale s3b-eg-restore "${s3b_eg_restore_approval:?exact EG target-restore grant}"
k get hpa cma-agent-manager cma-event-gateway \
  -o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{.spec}{"\n"}{.status}{"\n"}{end}'
```

Claim only actually observed counts and transitions. After both legs restore
their policies and all terminating processes exit, start the S3 final30-minute
stable soak. Preserve transition errors independently of the final stable window.

## Abort, incident delivery and drain evidence

All common safety/absolute aborts still apply immediately. Additional exercise
triggers are >20 inbound HTTP5xx in any rolling five minutes; p95≥2× the matched
ten-minute baseline for two consecutive minutes; any new above-baseline429 or
unknown429 cause; any restart increase, fewer than two Ready, unschedulable Pod,
OOM, rollout/template drift, failed HPA conditions/metrics, failed retiring-Pod
request, duplicate completion or unexpected target/production mutation. These
thresholds never defer an already-known safety, accepted-work or absolute failure.

Log01 immediately sends a safe incident to deployment01 with health01 copy and
own-pane `BLOCKED: URGENT`, using the [published incident/ACK route](stage-command-review.md).
Engineer01 explicitly ACKs the incident and action/status. Delivery exit0 is
not ACK; failed/denied delivery or missing ACK stays OPEN with immediate fallback.
Restore the original target under its approved grant, stop promotion and continue
observation. If restoration guards fail or service cannot recover safely, use
the approved S3 compatible recovery decision; never force a manual successful
trajectory. Keep all Deployments, Pods, PVCs, workspaces and user data intact.

Log01 uses the existing `drain_handoff`, `hpa_replica_timeline`,
`hpa_kubernetes_events`, `http_integrity` and `429_by_route_5m` queries with one
actual half-open UTC interval per leg and the matched baseline. Bind each retiree
UID/IP to request IDs/fingerprints, start/complete times, path class/status/latency,
drain-start/in-flight/drain-complete/SIGTERM/process exit, endpoint removal and
successful survivor traffic. Include requests already in flight at drain-start;
retiree-window request starts alone omit them. Count successes, 5xx/429, transport
failures/timeouts and duplicate/stale effects separately. Missing lifecycle or
upstream identity leaves the join unresolved. Empty collectors and absence of
failure counters alone never prove successful completion or zero duplicates.

Record each lower/restore source and result, original/temporary target, baseline,
observed scale/drain timings, exact counts/coverage, incident ACK/recovery and
remaining unknowns. No S3b production execution is claimed by this document.
