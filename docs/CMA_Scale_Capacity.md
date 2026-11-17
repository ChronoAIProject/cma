# CMA scaling: capacity qualification

The #1487 decision below supersedes the replica selection and capacity envelope
in the historical #1956 evidence collected read-only on 2026-11-13 around
23:04–23:23 SGT. Neither is permission to activate HPA. Production gates remain
with deployment-engineer-01/02, the Atlas owner (#1565), performance-analyst-02
and solution-architect-04. Use
[the apply procedure](CMA_Scale_Apply.md) after #1370 and this manifest PR merge.

## API max-six decision and activation budget (#1487)

The selected manifests now allow API **2–6**, with CMAEG unchanged at **2–4**.
The #1487 runtime activation profile has `64 - 16 = 48` usable stream permits
per API process. Required Ready instances for 200 active conversations are
`ceil(200 / 48) + 1 = 6`; after one instance loss, `5 * 48 = 240`, leaving 40
ordinary slots. The 16-per-process continuity/control reserve is not counted as
ordinary capacity. The previous max4 allowed only `3 * 48 = 144` after a loss.
This calculation assumes qualified ownership distribution and compatible quota
activation; stricter ownership/admission caps still bind. In particular, do not
reuse the fixed-two cutover packet's `am_max_owned_agents: 10` for this target.
Public observers sharing a conversation do not each consume another runtime
conversation stream; the separate 300-public-stream, relay and request budgets
remain part of #1487 qualification, not a throughput claim from this HPA change.

**maxReplicas is permission to grow, not six Ready instances.** The fixed-two
minimum stays consistent with S1/S2 and the existing Deployment/PDB contract.
At the 200-active target, prove six actual Ready instances before admitting that
load and qualify CPU HPA response and owner redistribution. CPU utilization may
not reflect stream pressure; if it fails to maintain the required capacity, S3
holds for a separate approved scaling policy. No fleet admission gate is added.

### Node fit: the documented pool fails N+1 and 30% headroom

Use the eligible **control-plane shared pool** snapshot below: three nodes,
each 15.89 allocatable CPU and about 27.73Gi. The Singapore figures in
[#1544](https://github.com/ChronoAIProject/cma/issues/1544) (eight runtime nodes,
about 127 vCPU/458GiB, 44–46 running Sandbox pods) and the unqualified four-to-six
runtime-node scenario in [#1545](https://github.com/ChronoAIProject/cma/issues/1545)
are separate Sandbox pools. They cannot be added to the untainted API node budget.
Reserve existing NyxID/direct consumers, pending pods and warm capacity in their
own pools; paused Sandboxes do not substitute for 200 active conversations.

Current API requests/limits remain **4/8 CPU and 256Mi/8Gi per pod**. Six API pods
therefore request **24 CPU/1.5Gi** and may use **48 CPU/48Gi**. Four EG pods add
1 requested CPU/8Gi and 8 CPU/8Gi limits. The historical shared-node workloads
excluding API/EG request 14.425 CPU and approximately
`3 * 27.73 - (20.866 + 19.280 + 21.267) - 0.25 - 2 = 19.527Gi`.

| Envelope | CPU | Memory |
|---|---:|---:|
| Six API + four EG + other shared-node requests | 39.425 | 29.027Gi |
| Three-node allocatable total | 47.67 | 83.19Gi |
| Three nodes at 70% (30% headroom) | 33.369 | 58.233Gi |
| After losing one node, allocatable total | 31.78 | 55.46Gi |
| After losing one node, at 70% | 22.246 | 38.822Gi |

**The documented three-node pool does not fit.** Steady requested CPU already
exceeds the 70% budget; after a node loss it exceeds even raw allocatable CPU.
Six API pods alone exceed the surviving CPU headroom budget (`24 > 22.246`),
and their memory limits alone exceed its memory budget (`48 > 38.822Gi`).
At all service limits, API/EG need 56 CPU/56Gi before other workloads.

The request-only aggregate lower bound is
`ceil(39.425 / (0.70 * 15.89)) + 1 = 5` nodes, before extra DaemonSets and
fragmentation. This is **not a sufficient placement plan**: six API pods spread
over fewer than six nodes can lose two with one node, leaving only
`4 * 48 = 192` usable streams. Prove six distinct API hosts to retain five Ready
instances after a single node loss; the existing maxSkew1 spread is not strict
one-pod-per-node anti-affinity. Zone loss needs its own larger bound.

For a conservative limit envelope, even substituting other workloads' *requests*
for their unknown limits gives at least 70.425 CPU/75.527Gi. Its CPU lower bound
with node loss and 30% headroom is
`ceil(70.425 / (0.70 * 15.89)) + 1 = 8` nodes. Eight is not a qualified fit:
refresh other workload limits, per-node bin packing, pending/surge/terminating
pods, topology, DaemonSets and pod/IP slots. The configured shared-pool autoscaler
range (3–30 regionally) is not Ready or prewarmed capacity. No node-pool change
is included in this decision.

### Termination, namespace quota and database gates

Keep the fixed 120s downscale interval and qualify hard process/client lifetime
G independently of the 760s Pod grace. At proven G760, `ceil(760 / 120) = 7`
terminating processes must be budgeted: API `6 + 7 = 13`; EG retains its existing
conservative `5 + 7 = 12` allowance despite its max4 manifest. The guarded S3
planner rejects the old API12 budget. No HPA ramp overlaps a rolling release;
API full old/new generations plus surge also give `6 + 6 + 1 = 13`, conditional
on actual controller proof. Longer lifetime requires a larger envelope.

| Envelope (before other namespace consumers) | Requested CPU | CPU limits | Requested memory | Memory limits | Pods |
|---|---:|---:|---:|---:|---:|
| Six API + four EG steady | 25 | 56 | 9.5Gi | 56Gi | 10 |
| Thirteen API + twelve EG process allowance | 55 | 128 | 27.25Gi | 128Gi | 25 |

Add the measured namespace remainder (requests 0.025 CPU/64Mi, limits
0.25 CPU/128Mi, one pod), then refresh all consumers. The historical quota of
40 requested/80 limit CPU fails the process allowance. **The existing proposed
S3 raise to 64/128 CPU also fails: `128 + 0.25 = 128.25 > 128`.** Keep S3 HOLD
until the owner approves and qualifies a sufficient quota and updates its
guarded procedure; this change does not silently increase the quota. The full
requested CPU node envelope is `55 + 14.425 = 69.425`, requiring at least
`ceil(69.425 / (0.70 * 15.89)) + 1 = 8` nodes before the same placement/limit
qualifications. It supersedes the historical six-node request-only estimate.

Retain the per-server Mongo formula below. At illustrative effective EG pool10,
steady six API/four EG cost `6 * 70 + 4 * 12 = 468` connections before other
consumers O. The full process allowance costs `13 * 70 + 12 * 12 = 1054`;
require `L >= ceil((1054 + O) / 0.70)` (at least 1506 even at O=0), with actual
driver topology, pool caps and Atlas limit attested. Unknown limits, insufficient
headroom or unqualified provider budgets continue to HOLD S3.

The remaining sections preserve the dated #1956 evidence and earlier max-five
calculations for provenance. Use the #1487 bounds above and the current staged
procedure for new activation packets; historical measurements are not fresh gates.

## Measured resources and provisional CPU targets

Both live Deployments were one desired/Ready replica, no HPA/PDB. Kubernetes
1.35.8 exposes an Available `v1beta1.metrics.k8s.io`; `kubectl top` works.
The existing rolling policy is maxSurge one/maxUnavailable zero, grace 735s.
Both canonical GCPBackendPolicies use 660s draining/timeout. Neither deployment
had topology spread; existing hostname anti-affinity is preferred.

| Service | CPU request / limit | Memory request / limit | 24h CPU p95 / max, cores | Provisional HPA |
|---|---|---|---|---|
| AM | 4 / 8 | 256Mi / 8Gi | 5.839 / 6.029 | 60% = 2.4 cores |
| EG | 250m / 2 | 2Gi / 2Gi | 0.3697 / 0.3964 | 200% = 500m |

Keep **all resources unchanged** for matched N=1/N=2 tests. AM's current 4-core
request leaves burst headroom to its 8-core limit; reducing it from a quiet
snapshot is unjustified given the historical 6-core load. Observed 24h aggregate
maximum working sets were about 184Mi AM / 24Mi EG. They do not cover all media,
concurrency and failure workloads: preserve the existing memory envelopes and
test memory pressure/OOM separately. Memory is not an HPA signal in this release.
EG's 200% utilization is valid: its CPU limit exceeds its request. A 60% target
would be 150m, below its observed background floor, potentially scaling to max
without useful extra throughput. The 500m proposal is about 1.26 times the
observed 396.4m maximum; **historical maximum is not stable idle p99**.

These are architect A1 values retained by A2, not measured latency knees. Require
target >=1.25 times representative idle p99 **and** below the measured latency
knee on the same final candidate. AM CPU includes worker/reconciliation work;
the 24h window spans image changes. HPA may need three AM replicas at historical
load; minimum two is a floor, not a promise that two handles every workload.

VictoriaMetrics query source is the read-only Kubernetes API Service proxy:
`/api/v1/namespaces/monitoring/services/vmselect-vm-stack-victoria-metrics-k8s-stack:8481/proxy/select/0/prometheus/api/v1/query`.
Use URL-encoded PromQL. Duplicate scrape targets are removed with `max`, rather
than summing both copies. The per-replica upper envelope avoids summing old/new
pods during a rolling update:

```promql
quantile_over_time(0.95,
  (max by (container) (rate(container_cpu_usage_seconds_total{
    namespace="chrono-bot",container=~"api|cmaeg"}[5m])))[24h:5m])
```

Use `max_over_time` for the corresponding maximum. CPU samples at 23:08 SGT
were about 1.119 AM / 0.339 EG cores. Direct pod `:9091/proxy/metrics` captures
between 23:16:09 and 23:22:40 SGT, same hashed process identities, recorded
AM HTTP attempts 3.249/s; EG event and console counter deltas were zero over
390s. These counters are not a clean user-request denominator; AM attempts may
include health/internal/relay work and EG had no event sample. **No CPU per user
request or EG request capacity can be certified from these observations.** Use
performance-analyst-02's revision-stratified baseline and developer-17's offered
load fixture; subtract measured idle CPU only with matched windows, exclude
health/relay duplication, and report CPU seconds per admitted original request
with background CPU separately. Missing denominator is NOT_RUN, not zero cost.

### Stable N=1 baseline handoff

Performance-analyst-02 supplies a separate stable window on 2026-11-13,
15:22–15:52 UTC (23:22–23:52 SGT), both services desired/Ready1.
AM image `dc8548c4a73f2d582d5f7ccdb7e05741d42f569e12f67bcf68ee440948002990`;
EG image `f8f502aed8b5ea2c68924994c98f988c483dcf689d65998f4d94aeabdf696330`.
The analyst's A2 receipt associates batch4/source b0129514a through the lead's
release instruction; that is separate from an immutable build attestation.

| Service | CPU mean / p95, cores | Working set mean / p95, MiB |
|---|---|---|
| AM | 0.828 / 1.040 | 52.152 / 54.375 |
| EG | 0.293 / 0.338 | 20.826 / 21.668 |

There were4,551 header completions, including3,029 non-probe completions
(1.6828/s), and no observed5xx/429. The adjacent31-minute exporter capture includes
one before-header cancellation and two EG stream_lost outcomes; zero HTTP5xx is
not zero logical failure. AM's aggregate0.4921 CPU-seconds per non-probe header
includes background/worker CPU, so it is not marginal request cost or a capacity
slope. EG lacks a matched inbound-to-confirmed-outbound population. These results
support preserving resources and provisional targets, not HPA/max5 qualification.

Create/header p95=766ms (N5), detail234ms (N150), send654ms (N8), and lifecycle
stop265ms (N6) are sparse backend observations, not completed user journeys or
proven interrupt latency. Models1974ms/environment1550ms each have N1; usage and
important transcript variants are unobserved. Sandbox N1318 has p95/p99=580/892ms;
Workspace N1298 has305/492ms. Final-candidate N1 repeats, matched N2/scale/fault
windows and completed client journeys remain required.

The31-minute evidence independently confirms application30/reporting4/routing30.
Application checked-out gauges0–6 are not total open/idle sockets or Atlas
headroom. Of380,427 application checkouts,25 were at_limit with p95 bucket(0.5,1]s;
that stratum cannot qualify p99. No server-operation latency/limit is inferred.

Retained research provenance: `performance-analyst-02/cma-scale/baseline-n1.md`,
SHA256 `1153af97c6a7d8a9f0234145cdf19829769fa0c37a3e9209e427c2846139a78b`;
`compare_baseline.py`, SHA256
`5f6cda4797a469623be8b17e6c5f8140171ed0e7fff70876f4d66594c387a6ce`.
Import the full package into approved release evidence custody; the helper never
emits release PASS. Historical noise from another image is not a final-candidate
allowance. No additional telemetry collection was started here.

## Metrics prerequisites and owner items

Managed Prometheus is enabled at the GKE cluster. VictoriaMetrics tenant 0 has
container resource history but no `cma_*` / `cmaeg_*` application series. Direct
private exporters expose AM HTTP histograms/counters and Mongo pool gauges;
EG exposes event/phase/console metrics. No custom/external metrics APIService or
CMA-specific scrape object was found by the architect's read-only inventory.
An exporter, or Managed Prometheus being enabled, does not establish HPA access.

CPU resource HPA uses the existing metrics API and requests; it needs no new
IAM or Managed Prometheus installation. Before S3 check both containers have
fresh resource metrics, no adapter errors and useful demand response. Measure
HPA recommendation/current/desired/Ready/terminating separately from queue age,
owned work, actual request throughput and latency. Demonstrate missing/stale
metrics do not cause scale-down; inspect `ScalingActive`, `AbleToScale` and
`ScalingLimited`, not just replica count. Do not disrupt production metrics to
test failure; use the isolated qualification environment.

If I/O/session pressure breaches latency without CPU scaling useful capacity,
**S3 is blocked**. Owner items: approve application scraping and a supported
Prometheus/custom or external metrics adapter, its read-only service identity,
Workload Identity/IAM and quota/cost, then prove a real fresh per-pod API read.
Prefer admitted original requests or active owned work at the passing latency
capacity; do not equate idle socket count with active work. Retain CPU as a second
HPA metric, with the higher recommendation winning. No such infrastructure is
created by #1956.

## Bound active, surge and terminating processes

A2 fixes min2/max5; up stabilization0s, Max(+2 pods,+100%)/60s; down
stabilization300s, Min(-1 pod)/120s initially. If qualified p95 from drain
initiation through all-owner handoff AND successor service exceeds90s, extend
period to ceil(p95+30s), using the largest qualified service/cohort unless a
separate per-service review approves otherwise. Sparse/censored/failed or missing
timings hold S3. `.github/scripts/cma-scale-manifest-contract.rb --hpa ROLE P95`
only renders this arithmetic; it cannot qualify an evidence cohort. Periods
above Kubernetes'1800s maximum HOLD for a reviewed alternative, never clamp.
The faster up policy can move2->4->5 in two policy windows; reserve the full
qualified node/Mongo/provider envelope before the first increase. Correctness
must independently exercise N3 as well.

Grace includes preStop and is760s. **p95 is not a hard process lifetime.** Define
G as the enforced maximum time a departing process can retain clients, starting
at the downscale decision: include controller/deletion delay, preStop, all
HTTP/worker tails, delayed node termination and socket lifetime. For down period
D, the conditional terminating allowance is T=ceil(G/D). Repeated load oscillation
makes max-minus-min unsafe. Unbounded G means no qualified process envelope.

With a **proven effective G760s**, D120 permits seven terminating plus five
active, or twelve/service. Kubernetes grace760s alone does not prove G760s.
For example G=841 requires eight terminating/thirteen processes and130CPU limits
before other consumers, beyond the proposed128CPU quota. Measure/enforce all
components; increasing D from measured p95 requires recomputing T against the
independently established hard G. No nonzero deletion delay is silently omitted.

Rollouts and HPA ramps must not overlap: disable/delete HPA through the reviewed
procedure, wait for zero terminating processes, then allow one rolling release
at a time and wait for complete termination before another. A complete old
generation can overlap the new generation: five old + five new + one surge =
eleven/service, **conditional on actual controller proof**. Budget the larger of
these separately proven envelopes. PDB and maxSurge do not bound terminating
pods. Extra scale writers, deployment replacement or extending grace invalidate
the assumptions. Initial no-overlap cutovers need their separate fixed-two,
legacy-client-expiry and compatible recovery budget.

Missing nodes, blocked finalizers, zombies or failed termination may violate G.
The reviewed observer must detect the first exceeded process/client/lifetime
bound and stop promotion/further scaling through approved controls; a reactive
alarm is not a hard termination gate. Capacity must already cover every process
that can remain alive. If G and operating restrictions cannot be enforced,
S3 remains HOLD pending a tested termination-aware gate or other qualified bound.
No such gate is implemented by these YAML policies.

Debugger-03's timing report SHA256
`cc631c79305defc3aa6a32db6cdfb9ad304a0adb5013b6aaa94754029604d00f`
records zero controlled loaded AM/EG startup, drain, takeover or exit trials
and zero HPA decision/deletion trials. Installed local Pod metadata gives3s/4s
container-start-to-latest-Ready (N2/service), with5s probes and unknown live K.
That exploratory observation is not process-main/first-Ready, owned service or
a qualified handoff p95. Thus760s grace and the envelope remain **NOT_RUN**.
Discord bot leases, routed target claims and replica partitions are distinct
populations; early-drain204 proves neither socket close nor exact lease release
nor successor service. Require those timings and correctness proofs on the frozen
integrated candidate before activation.

## Namespace quotas and node pools

Measured `chrono-bot` quota: requests 40 CPU/80Gi; limits 80 CPU/160Gi; 50 pods.
Used: requests4.275 CPU/2368Mi; limits10.25 CPU/10368Mi; three pods. After removing
the one AM and one EG, other current consumers contribute requests0.025 CPU/
64Mi, limits0.25 CPU/128Mi, one pod. Refresh this subtraction before every stage;
do not assume future consumers or unrelated releases fit the old remainder.

| Envelope, each service | Requested CPU | CPU limits | Requested memory | Memory limits | Pods |
|---|---:|---:|---:|---:|---:|
| Fixed two | 8.5 | 20 | 4.5Gi | 20Gi | 4 |
| Max five | 21.25 | 50 | 11.25Gi | 50Gi | 10 |
| Max + surge, six | 25.5 | 60 | 13.5Gi | 60Gi | 12 |
| Max + surge + one terminating, seven (not a bound) | 29.75 | 70 | 15.75Gi | 70Gi | 14 |
| Full rolling generations, eleven | 46.75 | 110 | 24.75Gi | 110Gi | 22 |
| HPA oscillation bound, twelve | 51 | 120 | 27Gi | 120Gi | 24 |

Add other consumers to every row. Current quota already fails eight of each on
CPU limits (80 +0.25 >80). **Owner capacity item**: for the twelve-process envelope,
propose requests.cpu64 and limits.cpu128, retaining memory80Gi/160Gi and pods50,
subject to the fresh all-consumer budget and headroom approval. No quota manifest
or production change is included here. No tested autoscaling maxN can currently be certified. Even the conservative
max3 +7 envelope costs100 CPU limits before others; a tighter envelope would
need independent proof. Fixed-two fits the snapshot quota, but survivor load,
transition capacity and Atlas gates are still unqualified. Do not activate min=max2 and claim automatic scaling was delivered.

Three untainted `shared-node-16c32g` nodes, one per zone, each have15.89 allocatable
CPU/~27.73Gi. Snapshot free requested CPU by zone a/b/c:12.208/8.006/8.781;
free memory20.866/19.280/21.267Gi. Other pools carry sandbox NoSchedule taints and
are not AM/EG capacity. Current eligible free CPU totals28.995. Five steady each
needs17 additional requested CPU over today's one each, but one-node-loss and
termination overlap need more nodes. Other workloads on shared nodes currently
request about14.425 CPU after removing AM/EG, so the twelve-process envelope
requires about65.425 CPU plus new-node DaemonSets. Scheduler placement must check
per-node fragmentation, topology, memory and pod slots, not just this sum.

Shared node autoscaler is enabled, BALANCED, min1/max10 **per zone** across three
zones (regional range3..30). Its configured maximum is not warm capacity or a
proof of Compute quota, IP range, stock or provisioning latency. Reserve at least
six such nodes for the twelve-process envelope plus one-node loss, then recalculate
DaemonSets/other workload growth and zone-loss requirements. Validate real
pending-pod scale-up and timing before S3; prewarm tested spare capacity for
the <=30s service recovery requirement. No node-pool changes are authorized here.

## Mongo and external provider budgets

Direct AM metrics `cma_mongo_pool_max_connections{pool=...}` showed
**application30, routing30, reporting4** on one unchanged process. Composition
creates a separate routing client; reporting remains separately bounded with
its pressure circuit. That is64 data connections/server plus a provisional six
monitoring sockets/server (two/client), or **70 per AM per server**. Confirm
monitoring topology/read preference from the exact driver/candidate. The old
#1370 cap10 cannot override live30 without explicit performance qualification.

EG's `serve_storage.rs:49` creates one serving client shared by record kinds;
the separate `:140` helper belongs to operator commands, not a second concurrent
serving client. The locked3.9.1 driver's audited default is10/server, but a URI
override can change it. Its effective cap is **unverified** without secret reads;
obtain credential-free startup telemetry/owner attestation or explicit reviewed
configuration, not the URI. Budget any operator jobs and temporary migration
clients separately. No Secret read is needed or permitted for this task.

For each server, with P=effective EG cap, O=all other consumers plus operations,
and L=server connection limit:

`B = active_and_terminating_AM * 70 + active_and_terminating_EG * (P + 2) + O`

Require `B <= 0.70 * L` with the owner-verified tier/per-server limit and a
complete conservative inventory, plus workload connection/memory/CPU evidence.
A2 permits the complete configured socket bound to establish >=30% connection
headroom when Atlas telemetry is unavailable; unknown tier or other consumers
still HOLD. With illustrative P10, two/five/twelve each
cost164/410/984 connections/server before O. Twelve requires
`L >= ceil((984 + O) / 0.70)`; e.g.1406 even if O=0. With P30 it is1224 before O.
Do not compare all-server sums to one server's limit, or forget monitoring
sockets on secondaries and independent read-preference pools. Server limit,
actual topology, O and the >=30% Atlas evidence are unresolved owner gate #1565.
Unknown limit means **S3 HOLD**, not assumed spare capacity. S1/S2 also need their
own measured two/survivor and transition budget before activation.

Provider capacity also multiplies per-process semaphores/workers. Developer-15's
inventory must supply global Discord Identify/reconnect reservations, bot/API
rate limits, partition fairness, connection ownership and receipt reconciliation
for every enabled native/relay/custom path. Do not multiply provider-global
quotas by replicas or blindly replay Unknown/Partial external sends. Live routing
includes EG console/legacy-console; cross-pod session evidence cannot be waived
on an assumption that console is unreachable.

Debugger-01's source inventory, SHA256
`6ac39157955e0af28e61d63d59d93668e2c0a343f506a55b1cf5069b1eec5ba9`, independently
supports these client counts at develop a85fb1b98 versus old integration d6fc953a6.
Re-run it on the final merged source. Locked Mongo3.9.1 pool caps do not bound
checkout waiters; retain admission/deadlines/cancellation and test queue pressure.
EG256 inflight is per provider, not a global process cap; media/STT/destination
pacing also multiplies. Shared report worker/source leases remain2/2 with queue8:
additional polling/pools do not increase that shared work ceiling. These are
source observations, not throughput or production configuration qualification.
