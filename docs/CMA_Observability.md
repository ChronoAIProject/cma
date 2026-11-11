# CMA application observability

This is the CMA-side implementation and integration contract for #1470. It
reuses the platform's VictoriaMetrics, VictoriaLogs, vmalert and Alertmanager.
It does not deploy another collector or change chrono-probe. Code and manifest
changes alone do not establish that production scraping or notification works.

## Collection and access

Agent Manager exports Prometheus text at `GET /metrics` on
`CHRONO_METRICS_BIND` (default `0.0.0.0:9091`). This is a separate HTTP listener,
outside the public router, authentication middleware and public Service ports.
It serves no business API. Startup fails if its address cannot be bound. A
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
already has its own listener and metrics; only its discovery metadata changes.
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
| Authentication | `cma_auth_resolutions_total{outcome}` and `cma_auth_resolution_seconds`: ambient subject resolution is `resolved`, `anonymous`, or `unavailable`. Worker/resource-specific auth paths and intentional auth bypasses are not included; HTTP results still cover their responses. |
| Readiness | `cma_readiness_check_success` is the last completed Mongo check's result. `cma_readiness_check_timestamp_seconds` supplies freshness; `cma_readiness_check_seconds{outcome}` reports check duration. A stale successful check is not ready. `/ready` retains its existing five-second freshness limit, `/livez` retains process liveness semantics. |
| Mongo pool | `cma_mongo_pool_events_total{operation,outcome}`, `cma_mongo_pool_operation_seconds`, `cma_mongo_checkouts_pending`, `cma_mongo_connections_tracked`, `cma_mongo_pool_max_connections`, `cma_mongo_connection_tracking_overflow_total`. These observe the general application client across its server pools; max connections is a per-server pool limit, not a fleet denominator. Tracking is bounded at 4096; overflow means tracked occupancy is incomplete. Command monitoring stays disabled. |
| Mongo reads | Existing `cma_mongo_read_{operations,failures,documents,document_bytes}_total` snapshots, by bounded collection and `find`. They cover instrumented reads, not every database command or wire byte. |
| Ledger reads | Existing `cma_agent_log_scan*` counters, by fixed caller/worker step/outcome, exported from periodic diagnostics. They measure read work, not accepted commands. |
| Workers | `cma_worker_alive`, `cma_worker_heartbeat_timestamp_seconds`, `cma_worker_progress_timestamp_seconds{phase}`, `cma_worker_operations_total{phase,outcome}`, `cma_worker_local_queued`, `cma_worker_local_active`. Instrumented schedulers are Agent commands, Sandbox commands, Agent connection/lifecycle work, launches and native warm pools. A guard clears alive/queue gauges on return, abort or panic. Heartbeat proves scheduler-loop activity; successful discovery proves a scan completed; neither proves a turn completed. |
| Optional background work | `cma_background_active`, `cma_background_deferred_total`, `cma_background_attempts_total{outcome}`, `cma_background_attempt_seconds`, `cma_background_pending_since_timestamp_seconds`, `cma_background_completion_timestamp_seconds`. Pending timestamps continue to age during a blocked operation. Zero means no observed pending sweep. These cover local resumable sweeps, not the oldest durable command in Mongo. |
| Runtime ledger holds | `cma_runtime_ledger_holds_total`: observed runtime capacity holds. This does not invent an occupancy estimate for runtimes lacking that contract. |
| Collector health | `cma_telemetry_sample_timestamp_seconds` is refreshed by the existing 10-second diagnostics worker. A fresh scrape with an old sample timestamp means that local worker is stalled. |

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
Required follow-up includes verified consumer attribution, durable projection
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
| Slow/blocked checkout | Histogram quantiles from `cma_mongo_pool_operation_seconds_bucket{operation="checkout"}`, failed checkout rate and `cma_mongo_checkouts_pending`. Evaluate an injected saturation, not just a static threshold. |
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
   provider permits, the complete warm-capacity inventory, browser fatal-event
   collection, or Edge Gateway queue/delivery instrumentation beyond its
   existing metrics. Those are explicit gaps, not zero-valued successes.

The local tests verify private routing, bounded HTTP labels, cancellation,
response bytes/trailers, stream observation classification and worker guard
cleanup. They cannot prove production scrape coverage, two-location execution,
the seven-day service window or real notification delivery.
