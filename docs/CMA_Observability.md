# CMA application observability

This is the CMA-side implementation and integration contract for #1470. It
reuses the platform's VictoriaMetrics, VictoriaLogs, vmalert and Alertmanager.
It does not deploy another collector or change chrono-probe. Code and manifest
changes alone do not establish that production scraping or notification works.

## Collection and access

Agent Manager exports Prometheus text at `GET /metrics` on
`CHRONO_METRICS_BIND` (default `0.0.0.0:9091`). This is a separate HTTP listener,
outside the public router, authentication middleware and public Service ports.
It serves no business API. A separate five-second upkeep task aggregates raw
histogram samples even when no collector scrapes or the listener fails; both
tasks stop with the server guard. Startup fails if its address cannot be bound. A
runtime listener failure is logged and detected by the external scrape/no-data
rules; it does not stop application traffic or active work.

Agent Manager and Event Gateway pod templates declare the existing chrono-cd
discovery contract:

```yaml
labels:
  chrono-cd.chrono-ai.fun/metrics: "true"
annotations:
  chrono-cd.chrono-ai.fun/metrics-port: "9091"
  chrono-cd.chrono-ai.fun/metrics-path: /metrics
```

These are pod-template labels, never Deployment selector labels. Event Gateway
already has its own listener and metrics; delivery and queue observations extend that registry.
The platform collector must select namespace `chrono-bot` as well as tenant
namespaces and stamp `cluster`, `namespace`, `pod`, `environment` and service
identity on samples. Verify this scope and discovery in the collector's active
configuration; adding metadata cannot prove that the target has been selected.
Set a 15-second scrape interval and a bounded timeout. Scrape every pod rather
than a load-balanced Service. Deduplicate overlapping discovery jobs, not pods.

Reuse the platform's cAdvisor/kube-state-metrics collection for CPU, resident
memory, restarts and desired replicas. There is no second process-resource
collector in CMA. Application counters add observations the infrastructure
collector cannot derive. chrono-probe separately checks authenticated service
reachability and readiness from outside the application.

There is no HTTP authentication on the scrape listener. A separate port and
absence of a public route prevent normal public routing, but are not a network
ACL. The platform network policy must admit the monitoring namespace on 9091
and restrict other access according to its workload policy. Do not publish this
port through a LoadBalancer, NodePort, public HTTPRoute or NyxID service route.

## Exported observations

Labels are bounded by code: route templates, standard HTTP methods (unknown
methods collapse to `OTHER`), status codes, stream kinds, collection inventory,
worker names, phases and outcome categories. Never label by raw URL, query,
request ID, owner, Agent/Sandbox ID, command ID, credentials or error text.
Unknown routes use `unmatched`. Correlation IDs belong in restricted logs.

| Family | Metrics and meaning |
| --- | --- |
| HTTP attempts | `cma_http_attempts_total{route,method}` starts before handler execution; includes attempts later cancelled before headers. `cma_http_in_flight` lasts until headers or cancellation. |
| HTTP responses | `cma_http_responses_total{route,method,status}` counts constructed responses. `cma_http_response_headers_seconds` measures handler-to-header construction, not network delivery. 401/403/429/5xx remain distinct. |
| Response bodies | `cma_http_response_body_bytes_total` counts bytes polled into the server body consumer. `cma_http_body_closed_total{reason}` and `cma_http_body_lifetime_seconds` report `eof`, `error`, or `dropped`. Body errors and drops after HTTP 200 remain observable. Trailers and backpressure are preserved. |
| Before-header cancellation | `cma_http_before_headers_cancelled_total` and `cma_http_cancelled_before_headers_seconds`; a client disconnect or cancellation is an observation, not proof of platform task failure. |
| SSE | `cma_sse_observation_closed_total{route,reason}` reports transport observation endings. It does not inspect protocol payloads or assert a terminal. `cma_public_streams_active{stream_kind}` and `cma_public_stream_polls_total{stream_kind,outcome}` export existing bounded stream diagnostics every 10 seconds. |
| Authentication | `cma_auth_resolutions_total{path,outcome}` and `cma_auth_resolution_seconds` cover ambient, worker and exact resource-request verification, including early rejection and cancellation. Independent routes record `bypassed`. Resolution is distinct from later endpoint authorization. |
| Readiness | `cma_readiness_check_success` is the last completed Mongo check's result. `cma_readiness_check_timestamp_seconds` supplies freshness; `cma_readiness_check_seconds{outcome}` reports check duration. A stale successful check is not ready. `/ready` retains its existing five-second freshness limit, `/livez` retains process liveness semantics. |
| Mongo pool | `cma_mongo_pool_events_total{pool,operation,outcome}`, `cma_mongo_pool_operation_seconds`, `cma_mongo_checkout_started_total`, `cma_mongo_connections_tracked`, `cma_mongo_pool_max_connections`, `cma_mongo_connection_tracking_overflow_total`. Fixed pool labels separate `application`, `reporting` and `routing` clients, each across its server pools; max connections is a per-server pool limit, not a fleet denominator. The reporting observer composes with its existing circuit callback. Tracking is bounded at 4096; overflow means tracked occupancy is incomplete. Checkout starts are events, not queue depth: the pinned driver may omit the end event on cancellation, so starts minus ends is invalid. Duration samples cover completed checkouts only. Command monitoring stays disabled. |
| Mongo reads | Existing `cma_mongo_read_{operations,failures,documents,document_bytes}_total` snapshots, by bounded collection and `find`. They cover instrumented reads, not every database command or wire byte. |
| Ledger reads | Existing `cma_agent_log_scan*` counters, by fixed caller/worker step/outcome, exported from periodic diagnostics. They measure read work, not accepted commands. |
| Workers | `cma_worker_alive`, `cma_worker_heartbeat_timestamp_seconds`, `cma_worker_phase_success_timestamp_seconds{phase}`, `cma_worker_operations_total{phase,outcome}`, `cma_worker_local_queued`, `cma_worker_local_active`. Instrumented schedulers are Agent commands, Sandbox commands, Agent connection/lifecycle work, launches and native warm pools. A guard clears alive/queue gauges on return, abort or panic. Heartbeat proves scheduler-loop activity; successful discovery proves a scan completed; neither proves a turn completed. |
| Optional background work | `cma_background_active`, `cma_background_deferred_total`, `cma_background_attempts_total{outcome}`, `cma_background_attempt_seconds`, `cma_background_pending_since_timestamp_seconds`, `cma_background_completion_timestamp_seconds`. Pending timestamps continue to age during a blocked operation. Zero means no observed pending sweep. These cover local resumable sweeps, not the oldest durable command in Mongo. |
| Runtime ledger holds | `cma_runtime_ledger_holds_total`: observed runtime capacity holds. This does not invent an occupancy estimate for runtimes lacking that contract. |
| Collector health | `cma_telemetry_sample_timestamp_seconds` is refreshed by the existing 10-second diagnostics worker. A fresh scrape with an old sample timestamp means that local worker is stalled. |

### Request attribution

`cma_http_classified_attempts_total{route,method,consumer,protocol}` is emitted
when headers are constructed or the attempt is cancelled, once verified auth
attribution is available. The original attempt counter still starts before auth.
`cma_http_classified_responses_total` adds status;
`cma_http_classified_headers_seconds` measures header construction latency.
`consumer` is a trusted credential path: `public_user` is retained provider-mode
browser authority, `direct_api` is an independently verified bearer/binding key,
`nyxid` is verified proxy or exact request-bound authority, `internal` is the
verified Gateway service path. Local trusted mode, rejected/anonymous requests
and independent routes remain `unknown`. These labels do not identify a client
application. Origin, User-Agent and caller-supplied consumer headers confer no
attribution. Protocol is derived from the route template (`agui`, `responses`,
`rest`, `auth`, `other`). Environment comes from the collector's target labels.

### Durable worker snapshots

`cma_worker_durable_items{worker,state}` and
`cma_worker_durable_oldest_timestamp_seconds{worker,state}` summarize persisted
work every 30 seconds. Agent commands retain separate `queued`, `in_progress`,
`waiting_action`, `completed`, `failed` and `cancelled` populations; only `expired_queue` compares a queued
command's actual queue deadline with now. Sandbox commands expose `queued` and
`expired_queue`. Launch `due` and Agent `lifecycle_due` reuse the scheduling
predicates, excluding live leases, future retries and non-retryable holds.
Their oldest timestamp is eligibility time (including expired lease/retry), not
admission time. Agent lifecycle is a subset of the `agents` worker's scope.
Queued is a candidate population, not proof that per-Agent lane ordering or
permissions allow execution. Waiting action and long running inference never
become expired queue merely because their admission is old.

Each Mongo aggregate has a one-second server limit, a two-second client limit
and indexes over the active population; sampling is sequential and skips missed
ticks. A scrape reads cached gauges only. Failed or timed-out samples preserve
prior values and set `cma_worker_durable_sample_success` to zero. Always gate
on success and fresh `cma_worker_durable_sample_success_timestamp_seconds`;
`cma_worker_durable_sample_timestamp_seconds` records the last completed attempt.
A count is absent until first success. Oldest zero means empty only alongside
a successful fresh sample and count zero. Compute age using `time() - oldest`
only when count and oldest are positive. A warm target scheduled at epoch zero
has no reconstructed eligibility timestamp; its positive due count remains
visible, with oldest zero explicitly unknown. These are shared database observations repeated by
overlapping replicas: do not sum them across pods. Use one fresh source or max
with the correct deployment/database scope.

Worker heartbeat and `phase_success` prove loop/phase activity, including idle
scans. They never assert durable advancement. Warm executions distinguish
`idle`, `attempted` and `error`; provider apply outcomes have their own counter.

### Warm capacity and reporting circuit

Every 15 seconds a separate task reads bounded deployment-scoped warm targets
and provider inventory under an eight-second total timeout. Fixed `kind=agent`
and `kind=machine` labels expose `cma_warm_pool_desired_replicas` and
`cma_warm_pool_ready_replicas` for current immutable image/Pod/home plans,
excluding retired/retiring plans. A ready count requires exact original binding,
observed controller generation and matching desired replicas. A deliberately
disabled target with no binding contributes zero. Missing/replaced resources,
unobserved generations and failed inventory set
`cma_warm_pool_observation_complete` to zero and retain the previous ready count;
consumers must also check `cma_warm_pool_sample_success_timestamp_seconds`.
`cma_warm_pool_sample_timestamp_seconds` tracks completed attempts.
No pool/image/resource IDs enter labels. This is the provider's ready-replica
contract; claimed/available counts are not inferred. The same bounded target
snapshot publishes native warm `due` count/oldest with worker freshness gauges.

`cma_warm_pool_apply_total{kind,outcome}` follows the fenced finish write.
`cma_warm_pool_revision_applied_timestamp_seconds` advances only when a new
revision is successfully acknowledged; an idle or unchanged periodic re-apply
cannot refresh it. This observation still resets on process restart.

`cma_mongo_circuit_until_timestamp_seconds{pool="reporting"}` exposes the actual
atomic circuit deadline. Open means `until > time()`; it expires naturally
without another request. `cma_mongo_circuit_opened_total{pool,reason}` counts
pressure observations that open/extend it, and
`cma_mongo_circuit_rejections_total{pool}` counts refusals at its guard. These
metrics preserve the existing reporting deadline and recovery behavior.

All counters are process observations and reset on restart. Idle event-driven
series may be absent until observed; do not treat every absent event counter as
a telemetry outage. Initialize and monitor required readiness/worker heartbeat
series independently of request traffic. Long-lived body duration histograms
include buckets through one hour; the final bucket remains unbounded. No
deadline or terminal is manufactured for long inference.

## Accounting contract for #1486

Use these separate identities before computing an SLI:

1. **HTTP attempt**: one actual network request. A retry is another attempt.
   Merge edge/backend observations of that same attempt using correlation
   evidence, not by collapsing all requests with the same command key.
2. **Accepted work**: one durable admission. For Agent commands the existing
   command identity and permanent idempotency reservation provide the admission
   anchor. Other operation kinds need their own durable receipt mapping.
   Admission retries must not create another logical effect.
3. **Native completion**: runtime completion evidence. A retained receipt's
   cursor/hash alone does not establish a native completion timestamp.
4. **Durable/public terminal**: recorded terminal, available public replay,
   bytes handed to the HTTP body consumer and actual client receipt are
   different boundaries. An HTTP 200 or normal body EOF establishes none of
   these terminal assertions by itself.

Classify accepted work as a recorded terminal outcome, cancelled, held, or
unfinished with age; retain explicit unknowns when evidence cannot establish
the classification. Carry unfinished admissions across reporting windows and
associate late terminals with their original admission. Keep production's
168-hour window separate from staging/fault injections. Long reasoning stays
unfinished according to the public operation contract, without a blanket
30-second failure cutoff.

The current HTTP metrics are backend attempt observations, not the complete
edge-plus-backend request SLI and not a durable accepted-work ledger. Do not
advertise the final consumer/protocol/environment SLI or `unknown=0` from them.
Trusted credential-path attribution is now exported, with explicit unknowns. Required follow-up includes durable projection
and reconciliation, native/public terminal timestamps and edge-only access
evidence. IDs remain in durable records/restricted events, not metric labels.

## Alert integration

Deploy rules through the existing platform rule evaluator and notification
pipeline. Use 10-second evaluation and a notification budget of 10 seconds or
less for the critical acceptance path. Queries below show per-pod conditions;
scope each to the expected cluster/environment/namespace/service in platform
configuration. Attach an owner, runbook and bounded routing labels.

| Condition | Expression or rule contract |
| --- | --- |
| Mongo not ready or readiness monitor stale | `cma_readiness_check_success == 0 or (time() - cma_readiness_check_timestamp_seconds > 5)`; no additional multi-minute `for`. |
| Scheduler exited | `cma_worker_alive == 0`; enable only after startup qualification. |
| Child task terminated | A positive increase in `cma_worker_operations_total{outcome="terminated"}`; a scheduler may still be alive after a child panic. Investigate error outcomes separately from expected conflicts/retries. |
| Scheduler-loop stalled | `time() - cma_worker_heartbeat_timestamp_seconds > 15`; tune warm-pool thresholds against measured bounded reconciliation time. This is a diagnostic, not a liveness restart trigger. |
| Diagnostics stopped | `time() - cma_telemetry_sample_timestamp_seconds > 30`. |
| Slow/blocked checkout | Histogram quantiles from `cma_mongo_pool_operation_seconds_bucket{operation="checkout"}`, failed checkout rate and checkout-start rate. Exact pending checkout count/oldest age still need cancellation-safe instrumentation; do not derive them from starts minus ends. Evaluate an injected saturation, not just a static threshold. |
| Background sweep stuck | `(cma_background_pending_since_timestamp_seconds > 0) and (time() - cma_background_pending_since_timestamp_seconds > 120)`; local sweep age only. |
| Unexpected stream endings | Report `error`/`dropped` observations separately; protocol-specific terminal/error and client evidence must establish whether an ending was unexpected before using it in the durable terminal error budget. |
| Missing target/data | Compare successful, fresh targets to the expected Deployment replica inventory. A vanished pod/series must not shrink the denominator. Alert on a continuous 30-second gap; include scrape failures and missing heartbeat series. A query over only existing series cannot detect a completely vanished target. |

For example, with a scoped expected replica count, compare
`sum(up{job="<qualified CMA pod scrape>"}) or vector(0)` to the Deployment's
desired replicas and apply `for: 30s`. Also check collector health and expected
per-pod series freshness. Use the platform's actual job and kube-state-metrics
labels; a placeholder job name is not a deployable rule. During rollout, retain
all current pod targets and account for temporary surge explicitly.

With 15-second scrapes, 10-second evaluation, 10-second notification and a
30-second no-data threshold, worst-phase timing can exceed 60 seconds. Measure
fault-to-detect and detect-to-delivery separately; shorten the critical signal
path or use direct failure/event notification where the issue requires it.
Two consecutive 30-second external probes are a different qualification and
do not establish the critical under-60-second notification gate.

## Acceptance and outstanding integration

Before claiming #1470 complete:

1. Deploy a compatible image and verify `/metrics` on the pod's private port,
   and its absence on public routing. Confirm monitoring-network reachability.
2. Inspect the active platform targets: every expected Agent Manager and Event
   Gateway pod, 15-second samples, correct environment labels, no duplicate jobs.
   Query actual application series alongside platform CPU/memory/restarts.
3. Inject Mongo unavailability/checkout delay, scheduler exit or stall,
   stream drop/error and collector/target loss in an isolated environment.
   Compare raw event counts and test evidence with samples; retain actual
   fault/detection/notification timestamps and the notification receipt.
4. Qualify the external two-location authenticated body checks in chrono-probe
   and separately run the deterministic/funded-turn journey tests. Fixed
   synthetic deadlines must not become arbitrary-user inference deadlines.
5. Complete the remaining issue families and #1486 reconciliation. This change
   does not implement durable admission/terminal SLIs, native-to-public lag,
   cancellation-safe Mongo pending/oldest-age observations or
   claimed/available capacity beyond the provider ready-replica contract.
   Provider permits, browser diagnostics and EG queue/delivery
   observations are described below with their actual boundaries. Remaining
   gaps are explicit, not zero-valued successes.

The local tests verify private routing, bounded HTTP labels, cancellation,
response bytes/trailers, stream observation classification and worker guard
cleanup. They cannot prove production scrape coverage, two-location execution,
the seven-day service window or real notification delivery.

## Admission, terminal, browser and EG coverage

The CMA command-store admission boundary exports
`cma_admission_attempts_total{operation,outcome}` and `cma_admission_seconds`.
Ordinary reservations distinguish confirmed `created` and `existing`; interrupting
messages, targeted controls, queued-input edits and queued cancellation report
`confirmed` or the fixed typed rejection code. Unavailable results are
`unconfirmed`, not rejected work: a commit may already have happened. A dropped
future reports `cancelled`, not a durable cancellation. Internal transaction
retries do not create extra attempts. These are process observations, not the
unique accepted-work denominator. Trigger first-message admission inside a
larger transaction is represented by the durable snapshots, not an ordinary
reservation attempt.

`cma_admission_durable_commands{status}` reuses the bounded majority snapshot of
`agent_commands`, including all six statuses: `queued`, `in_progress`,
`waiting_action`, `completed`, `failed`, `cancelled`. Each document contributes
once regardless of request retries. This is the current retained population,
not a rate or 168-hour admission cohort. The same status appears on worker
snapshot success/freshness metrics; terminal populations are not pending work.
Oldest timestamps remain **admission** timestamps, including for terminal
populations; they must never be interpreted as completion times. Historical
records with malformed/missing admission timestamps fail that state's sample
rather than silently creating a valid zero. Gate each population on its fresh
successful sample and never sum these shared DB snapshots across AM replicas.

`cma_terminal_publications_total{outcome,confirmation}` counts new durable
`TurnCompleted`, `TurnFailed` and `TurnInterrupted` publications after an
acknowledged transaction commit, including a commit independently reconciled
by the existing snapshot proof. Covered paths are conversation updates,
message/reopen checkpoints, worker publications and direct log append.
Checkpoint response replay does not recount the retained terminal. Histograms
`cma_terminal_observation_to_commit_seconds{outcome}` measure AM event
observation/preparation (`appended_at`, second precision) to confirmed commit;
invalid/future timestamps increment `cma_terminal_timestamp_invalid_total`
instead of entering a negative latency sample. They do **not** measure delivery
into a browser or native completion → public terminal lag. Native completion
has no independent timestamp in this existing record contract:
`cma_terminal_native_timestamp_missing_total{outcome}` exposes that absence.
Crash between DB commit and counter update can lose an observation, and
concurrent ambiguous-commit reconciliation can observe the same publication;
these counters are diagnostic, never the exactly-once SLI ledger for #1486.

The web bootstrap loads `/browser-telemetry.js` before the application module
and reports script/style load errors, uncaught errors, unhandled rejections,
React caught/uncaught errors and Vite chunk preload errors. It emits at most one
report per category per page (five total), with no retry. No exception message,
stack, source URL, page path, browser identity, token or resource ID leaves the
browser. Network failure of the reporter is swallowed. A page whose bootstrap
script or network fails entirely may emit nothing; external asset/journey
checks remain required.

`POST /api/v1/telemetry/browser` is an anonymous, public diagnostics contract
for any client, documented in OpenAPI. Its JSON contains only one enum `kind`;
additional fields and unknown values are rejected, body size is <=256 bytes,
and a local 20 reports/sec process budget runs before body parsing in addition
to the normal network admission guard. This route bypasses auth, Mongo and
activity recording so startup/auth faults can still be reported. Valid reports
increment `cma_browser_fault_reports_total{kind}`; exhausted budget increments
`cma_browser_fault_reports_limited_total` and returns 429/Retry-After. They are
untrusted client observations, not a fatal-session rate or availability SLI.
The existing public `/api` Gateway prefix carries this diagnostic endpoint;
`/metrics` remains private.

EG adds `cmaeg_delivery_queue_depth{stage}`, capacity and sample timestamps for
its `inbound` and aggregate `worker` queues. Production uses guarded entries at
both stages: residence starts after capacity reservation and ends at dequeue or
receiver loss/panic. Depth excludes blocked producers and active sends.
`cmaeg_delivery_queue_oldest_seconds{stage}` continues aging at one-second
intervals while consumption is blocked; `cmaeg_delivery_queue_residence_seconds`
samples successful dequeue. Waiters, `wait_seconds` and
`submissions_total{stage,outcome}` distinguish accepted, full, closed and cancelled
producers without double counting. Gate snapshots on fresh
`cmaeg_delivery_queue_sample_timestamp_seconds`. The legacy raw-channel scheduler
constructor retains sampled depth/capacity only, without oldest/residence.
These local queues do not represent all durable/routed turns.

Provider concurrency observations use the configured allowlisted `provider` slug,
with no request identity. `cmaeg_provider_permit_capacity` is the smaller of the
provider limit and deployment limit **for each provider**, not a shared global
pool. `cmaeg_provider_permits_in_use` and `cmaeg_provider_permits_available`
track held permits and configured capacity minus held permits.
`cmaeg_provider_permit_waiters` counts acquisition attempts still pending;
`cmaeg_provider_permit_oldest_wait_timestamp_seconds` is zero when empty.
For nonempty waiters, query `time() - oldest_wait_timestamp_seconds` for current
age, allowing for wall-clock skew. `cmaeg_provider_permit_acquisitions_total`
and `cmaeg_provider_permit_wait_seconds` classify acquired, full, timeout, closed
and cancelled attempts. `cmaeg_provider_permit_held_seconds` measures the original
permit lifetime, including streaming. Cancellation, request errors, disconnects
and task unwinding release ownership. Existing 50ms waits and immediate admission
remain unchanged; a long held permit alone is not evidence of a dead worker.

Mongo checkout callbacks expose no per-acquisition identity, and dropping an
acquisition future can omit the ending event. Starts minus ends would leak
phantom pending work, so exact pending/oldest metrics remain unavailable until
there is a supported cancellation-safe driver boundary. Do not substitute
application operations in flight for driver checkout waiters.

`cmaeg_delivery_outcomes_total{platform,step,destination,outcome}` reports
bounded logical delivery-step outcomes for native/relay transports, actual
custom webhook legs, default destinations, and local stream buffering.
Compatibility aggregate callbacks do not double count custom delivery.
`confirmed` is distinct from `unknown`, `rejected`, `rate_limited`,
`unsupported`, and local `buffered`/`unavailable`. A buffered local frame is
never external recipient confirmation. A webhook group default destination is
a logical group result, labelled `webhook_group`, not a per-recipient receipt.
Existing delivery duration metrics remain in place. EG turn duration buckets
now include 30s, 120s, 600s, 1800s and 3600s, without imposing a turn deadline.

External journeys and production collection/notification acceptance are tracked
in [chrono-probe #18](https://github.com/ChronoAIProject/chrono-probe/issues/18).
It reuses the existing collector and probe checks, and coordinates versioned
rules/dashboards in the platform repository. Creating it neither closes #1470
nor implements #1486's complete durable request/terminal accounting.

Worker supervision, progress-aware stall detection, safe recovery and readiness
are tracked separately in [CMA #1740](https://github.com/ChronoAIProject/cma/issues/1740),
a child of #1470. This metrics PR does not change worker restart or readiness
policy; long inference and user waits must not become arbitrary failure deadlines.
