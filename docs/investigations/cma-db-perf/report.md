# CMA MongoDB checkout investigation — 5 November 2026

Issue: https://github.com/ChronoAIProject/cma/issues/1320. Investigator: `cma-db-perf`.
Production source: `1f345ff2cf6016972ae00912ea19ee58327d9f07` (#1311), deployed November 4 at 17:56 SGT. All incident timestamps below are UTC. Investigation uses read-only Kubernetes API-proxy GETs to VictoriaLogs/VictoriaMetrics; no pod exec, production database connection, writes, messages or recovery actions.

## Finding and limits

**There is proven, avoidable application query amplification competing for a ten-connection Mongo pool.** This PR removes serial task-status queries, adds their missing event-kind index, and throttles repeatedly unsuccessful optional maintenance checks. #1316 separately removes repeated complete-history validations in event-page reads. Both changes matter; this PR does not edit #1316's files or duplicate its range-read implementation.

**The historical telemetry cannot establish whether every pool connection was occupied during the extreme November 3 stall, identify the commands holding connections, or establish a database capacity shortage.** Checkout summaries omit connection occupancy and hold duration; command monitoring is intentionally disabled because this driver eagerly decodes command/reply bodies. No MongoDB exporter series are available among the 2,104 metric names returned for October 28–November 5 in the accessible VictoriaMetrics inventory. The patch adds bounded, content-free connection occupancy/hold summaries without enabling command monitoring. It does not claim that the measured code changes completely explain or cure every production delay.

**The agt_0a51a71d6472407ba348d95667b398d7 502 is a distinct demonstrated failure boundary.** The runtime generated it on a failed outbound `reqwest::Client::execute` to the model proxy, before an HTTP response existed. That runtime does not use Agent Manager's Mongo pool. The contemporaneous checkout data is materially different from the November 3 stall. A common external network cause remains untestable with the available evidence; asserting the same Mongo root cause would be incorrect.

## Production measurements

The prior latency report supplies complete four-hour cohorts (all checkout summaries and HTTP responses, approximately ten-second summary-boundary uncertainty):

| Window | Checkout count | Checkouts/s | Mean checkout ms | Approx. outstanding checkout demand, rate × mean wait | API responses |
| --- | ---: | ---: | ---: | ---: | ---: |
| Nov 3 05:47–09:47 | 2,948,990 | 204.79 | 3.19 | 0.65 | 24,272 |
| Nov 3 09:57–13:57 | 4,347,831 | 301.93 | 3.61 | 1.09 | 22,887 |
| Nov 4 05:47–09:47 | 4,163,077 | 289.10 | 82.20 | 23.76 | 50,047 |
| Nov 4 09:57–13:57 | 5,404,240 | 375.29 | 34.97 | 13.12 | 61,283 |

Rate × wait is a steady-state Little's-law approximation, not observed pool occupancy; it includes checkout scheduling/establishment and is not an extra count of checked-out sockets. Counts include background work and streamed response activity, so dividing by HTTP responses does not yield per-request query counts. All four cohorts logged zero checkout failures; cancellation and application timeouts need not produce a checkout-failure event.

Mean checkout delay was 25.8× the previous day's same-hour baseline before deployment and 9.7× afterward. The post-deploy checkout volume increased 29.8%. API CPU averaged 3.38→3.22 cores, compared with 1.28/1.87 the prior day. No CPU-throttled periods were recorded in those four-hour cohorts. Snapshot 503s were 161/200 before and 454/498 after; the same four Agents had 140/143→451/469 failures. A deployment regression is not established by those aggregate error rates.

### November 3 agent-tools dependency stall

Newly recovered summaries extend #1301's evidence:

| Summary time | Window ms | Completed checkouts | Maximum checkout ms |
| --- | ---: | ---: | ---: |
| 16:30:31.188883 | 10,001 | 5,371 | 78.608 |
| 16:30:54.136190 | 22,947 | 4,915 | 14,500.142 |
| 16:32:24.422258 | 90,286 | 1 | 104,786.177 |
| 16:32:42.741457 | 18,319 | 18 | 104,788.981 |
| 16:32:52.745173 | 10,003 | 6,569 | 18,321.889 |
| 16:33:02.745233 | 10,000 | 7,751 | 298.761 |

The complete 16:30–16:34 summary cohort has 81,064 checkouts, mean 33.09 ms, maximum 104,788.981 ms, zero logged failures. Summaries are event-triggered, so a long silence means no summary-triggering completion, not zero pending operations.

The agent-tools HTTP 503s at 16:30:54.197897 and 16:31:09.545250 each took 15,001 ms. Source sets a 15-second request deadline and admits up to 32 concurrent tool requests. #1301's first `session/new` timed out after 30 seconds, followed by a 94,045 ms retry-state publication. HTTP tool records lack the Agent binding, so their ownership cannot be proved. Across 16:29–16:34, the connection reconciler completed 174 scans of 52 candidates, made 2,043 optional context checks, scheduled zero recoveries, and had one 124,003 ms scan.

cAdvisor one-minute CPU rates fell from 3.24 cores at 16:30:45 to 0.029 at 16:31:55 and 0.020 at 16:32:25. Thus CPU starvation from decoding is **not established as the cause of this extreme stall**; the process was largely waiting. The missing occupied-connection and server-health evidence prevents distinguishing long server operations, remote I/O stalls and pool availability/scheduling.

### November 4 model-proxy 502

The existing `cma-502.md` report established 30 transport warnings at 04:59:14.541812–05:02:05.014083 on the target runtime, with no upstream HTTP response. `%error` omitted the lower transport chain, so DNS/TCP/TLS/reset attribution is unavailable. No retry or sandbox restart was performed here.

In the covering 04:59–05:03 Agent Manager window, 24 checkout summaries contain 80,197 checkouts, mean 11.569 ms, maximum 1,934.458 ms, zero failures. API CPU was 2.07–3.47 cores in the sampled minute rates. This is unlike the 105-second control-plane checkout stall, and cannot explain an outbound runtime transport exception. Neither an infrastructure cause nor recovery of the target Agent is inferred from silence.

## Pool, concurrency and connection ownership

Read-only live deployment inspection found one Agent Manager replica, `TOKIO_WORKER_THREADS=4`, CPU request/limit 2/4 cores and memory request/limit 256 MiB/1 GiB. All Agent Manager stores clone the same Mongo client. Separate service processes have separate pools; rolling overlap can temporarily double API clients.

A filtered in-memory inspection of the configured URI retained only allowlisted options: scheme `mongodb+srv`, explicit `retryWrites=true`, no explicit max/min pool, maxConnecting, waitQueueTimeoutMS, socket timeout, load-balanced or read-preference override. No credentials or URI were printed or retained. The exact locked `mongodb` 3.9.1 archive was independently checksum-verified against Cargo.lock (`af84443ae9878f59a6818686f339064479f42cdf293c4c637d8fc9c0a8c5eed6`). Its `cmap.rs` defaults to **10 connections per server**, min pool 0, maxConnecting 2. It has no timeout around the checkout request/response channel. URI parsing retains the deprecated waitQueueTimeout option, but this version's pool checkout implementation does not enforce it; this deployment supplies none. Application deadlines remain relevant.

| Caller | Configured concurrency / cadence | Mongo use |
| --- | --- | --- |
| Agent lifecycle | 50 Agent tasks, plus one discovery and one recovery scan; one-second sweep | Shared pool; claims, reads, transactional publications and log validation |
| Agent tools | 32 requests, 15-second deadline | Setup authentication normally two reads plus transaction completion; mutations additionally fence lineage |
| Sandbox / Workspace lifecycle | Four Sandbox sweeps / one Workspace sweep, one-second cadence | Each sweep iterates due work; runtime/network work happens outside persistence transactions |
| Sandbox reconciler | One scan every 15 seconds | Durable authority and resource reads, plus runtime observations; no blanket transaction around network awaits |
| Command control worker | Eight concurrent operations, one-second discovery | Shared durable queues and fenced mutations |
| Titles | Four workers, five-second polling | Candidate reads and bounded/fenced title writes; full log validation in existing title paths |
| Suggestions | Two workers, one-second polling, 80-second outer bound | Mongo claim/context/settlement; model generation after claim, outside the completion transaction |
| Provenance backfill | One Agent at a time, 25-ID pages, 250 ms item delay, 300-second sweep pause, 30-second startup delay | One page query; per Agent normally provenance aggregate plus abort when already retained; missing rows add lineage fences and insert/commit |

Transactions use snapshot reads and majority writes. In this driver's non-load-balanced replica-set mode, a transaction does **not** reserve one physical connection across every application await: it borrows a connection for each command. Mongos pinning and load-balanced physical pinning are separate driver paths. A long transaction still causes more commands, retries, snapshots and contention; its wall duration must not be labeled socket hold time.

No Mongo change-stream/watch call exists in the inspected Agent Manager/core source. Browser streams and workers poll. Responses-v2 response observation can poll every 250 ms while nonterminal, invoking history projection and authorization; per-frame authorization is deliberate. Agent event pages use a different bounded-window path. This PR preserves those authorization fences and history-integrity contracts.

## Operations and avoidable work

Counts below are source-derived common-path database commands; cursor getMore, policy/authorization phases, retries, oversized-record fallback and conditional mutations can add commands. Historical logs do not provide a per-route command census.

| Route / operation | Existing work | This PR / other owner |
| --- | --- | --- |
| Workspace hierarchy GET | Three collection reads plus 2–3 serial task-status reads per ready Agent, then read-only transaction abort | One bounded status aggregation per ≤100 ready Agents, plus cursor batches/oversized fallback; original transaction retained |
| Owned/shared Agent provenance inventory | Candidate inventory and per-Agent provenance/channel evidence, then 2–3 status reads per ready Agent | Batch only status evidence in the same snapshot; provenance/access checks remain |
| Individual provenance GET | Normally one bounded seven-lookup provenance aggregation plus transaction abort; large-record sequential fallback | No new status query on this individual route; no change |
| Agent-tools setup discovery | Capability receipt find + provenance aggregate + transaction completion | Remains dependent on the shared pool; no full-history scan in the normal setup path |
| Event snapshot, P events from history H | P `agent_log(limit=1)` calls, each reading aggregate and validating H complete entries, plus cursor getMore and tail work | #1316 owns the range-read correction; untouched here |
| Conversation update / Agent work publication | Aggregate/worktree/guard reads, complete H-entry log integrity/capacity scan, fenced writes and commit; retries on known-aborted conflicts | Not relaxed. Complete-log checks defend corruption, sequence and capacity contracts; replacing them safely needs a separate persisted integrity design |
| Turn completion suggestion enqueue | Up to 128 bounded context events, Sandbox read and possible outbox upsert before commit; ordinary nonterminal deltas return immediately | Model/network generation is asynchronous. No evidence it explains pre-#1311 delay |
| Optional managed context reconciliation | Read Agent and parent, resolve live registry, ask for receipt; repeats on every completed sweep even when unchanged and receipt absent | Retry unchanged candidates at most every 30 seconds; changed Agent revision, conversation, runtime or desired revision is immediate. No authority cached; disconnected recovery bypasses this gate |

The status queries previously had only `agent_log_sequence(agent_id,seq)` available for event-kind predicates. A no-gap completed turn could inspect every intervening message. The new `agent_log_task_status(agent_id,record.kind.type,seq DESC)` index makes those event-kind/range lookups selective. This additive index costs storage and one extra index update per log insertion; production verification must include DB write latency once an exporter is available. The aggregation limits each joined evidence record to 64 KiB, using a marker and original sequential reads within the same snapshot for larger records, rather than exceeding Mongo's 16 MiB joined-document limit.

### Measured background work and transaction timing

| Four-hour window | Reconciliation scans | Mean candidates | Context checks | Recoveries scheduled | Mean/max sweep ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Nov 4 before | 5,091 | 51.54 | 76,136 | 0 | 2,267.74 / 14,761 |
| Nov 4 after | 8,088 | 48.32 | 119,304 | 0 | 1,206.88 / 8,671 |

These are actual completed-scan summaries, not configured cadence multiplied by time. Optional maintenance is a proven repeated load source predating #1311. Each admitted check reads the Agent; eligible checks also read its Sandbox. The maximum simple two-read estimate is 238,608 commands in the after window (4.4% of all checkouts), before conditional additional work. The code change does not imply a 30× reduction in overall database traffic; changed candidates and other callers remain active.

There were zero logged provenance-backfill pages with recorded/refused rows in either November 4 cohort. Successful retained rows are silent, so this cannot establish that the worker was idle. The one incident page at November 3 16:32:45 had recorded=0/refused=8; timeouts are not proof of corrupt provenance. Its serial/throttled design does not support blaming it for hundreds of simultaneous connections.

Slow-publication logs are thresholded at ≥100 ms or retry. Before: n=906; elapsed minus local writer-queue p50/p90/p99/max = 1,000/2,185/4,234/8,861 ms, eight retried publications. After: n=968; 533/1,520/2,553/3,484 ms, five retried publications. This subtraction isolates the period after the local writer queue, **not exact server transaction execution or socket hold duration**. The 18-minute retry gaps documented in #1299 match the lifecycle lease and `agent log capacity exceeded`; they are not 18-minute Mongo socket checkouts. The log has normal/stop/delete capacity budgets of 10,000/10,064/10,128 entries and 64/64.5/65 MiB. Raising those limits or bypassing validation is not justified.

## Expected contribution of #1316

#1316 replaces P repeated complete validations with one validation per bounded page and replaces the uncommitted-tail read with a point read. For the demonstrated H=1,286, P=100 case, validation visits fall from 128,600 to 1,286 per page (99% reduction); P=200 gives 257,200→1,286 (99.5%). Cursor getMore and full-record decoding within the remaining scan still exist. Its empty/partial-page contracts and byte limits remain intact.

That is an operation-count expectation, not a predicted production latency percentage. Snapshot requests already failing at 15 seconds may not have completed P scans. It does not remove full validation on writes, response-observation snapshots, lifecycle/title operations or unrelated authorization/list queries. We cannot convert it into a precise checkout-delay reduction from existing aggregate telemetry. Deploy #1316 with this PR and compare matched-history cohorts; never claim their combined improvement belongs solely to this patch.

## Local verification and measurements

The production-shaped Mongo test uses 20 ready Agents × 1,286 events with 4 KiB message entries, ten complete and ten with a gap. It executes the original sequential path after dropping only the new index on a disposable local replica set, then restores the index and executes the final batch path. Test command monitoring is local only. Both paths must produce identical statuses; command counts and index examination counts are assertions. Large evidence, snapshot isolation under concurrent appends, high-water disagreement, corruption and authoritative non-ready statuses have regression coverage.

Initial measurement (unoptimized test build, Apple silicon, loopback replica set): **60→1 commands; 32.752→6.016 ms**. The final pinned-toolchain run on the shared machine measured **69.442→9.085 ms**, again with 60→1 commands and 1,284→0 examined gap documents. These are local method measurements, not a production latency forecast. Final gate results are recorded in [verification.md](verification.md).

A deterministic paused-clock reproduction drives 15 unchanged eligible candidates through 300 one-second sweeps: **4,500→150 maintenance admissions (96.7% reduction)**. Revision/runtime/conversation changes and retry expiry are tested independently; a real worker regression proves connection loss still schedules immediate recovery. Tracking is bounded to 1,024 candidates and fails open on saturation.

Connection metrics track at most 4,096 ephemeral connection fingerprints. Hold timing survives reporting windows, checked-in and closed connections release tracking, and excess tracking is explicitly counted. No database, command body, credential, address or connection fingerprint is logged. The telemetry does not attribute sockets to routes. Occupancy combines this client’s server pools, so interpret the ten-connection threshold only in stable primary-only periods, not across failovers. Clocked hold time includes driver/runtime scheduling until check-in. Closed-in-use samples increment `failures`; they do not assert a particular Mongo command failure.

## Capacity recommendation and production verification

No infrastructure change or pool increase is made. Ten connections can be a bottleneck at 289–375 checkouts/s, but raising the limit without command-hold/server evidence can increase pressure on a constrained Mongo server. First remove the demonstrated work and deploy the bounded telemetry. If checkout waits remain high while in-use peaks repeatedly reach 10 in stable primary-only periods and hold p90 remains short, test a controlled URI `maxPoolSize=32` canary with the DB owner's server CPU/cache/disk/replication metrics. Revert if server latency/errors increase. If hold times grow together with server CPU/IO/cache/replication pressure, size the database tier/node against measured demand instead of merely increasing client concurrency. Server-side metrics need an authorized exporter; no direct database access is requested or attempted by this investigation.

The lead owns merge, batch deployment and issue closure. Verification must include:

1. Compare equal four-hour windows and same-Agent snapshot cohorts, excluding rollout. Recompute checkout count/rate, weighted mean `sum(total_us)/sum(count)/1000`, exact bucket counts, maximum and failures. Do not average per-window means.
2. Read the new `connection_in_use` summaries alongside checkout summaries: `in_use`, `in_use_peak`, `untracked`, duration buckets and closed-in-use failures. Expect `untracked=0`. A peak alone is not sustained occupancy; inspect consecutive summaries.
3. Compare `Agent connection reconciliation completed`: context_checks should fall for unchanged candidates, context_throttled should rise, context_current should eventually rise after successful maintenance, recovery scans must keep completing. Verify lost connections still schedule and publish; throttle delays optional unchanged-parent/runtime retry by at most 30 seconds, not user work.
4. Compare successful workspace/inventory GET p50/p90/p99 stratified by ready-Agent count; monitor 503/error rates separately. Verify mixed status/gap/queued-work presentation and owner/shared authorization remain correct. The added index is created during ordinary store startup, so include index-build readiness time in rollout observation; no schema/record rewrite occurs.
5. Compare agent-tools status/latency and OpenConversation startup phases. Require absence of repeated 15-second 503s and >15-second checkouts under comparable load, not simply absence of one error string.
6. Verify #1316 snapshots at 100/200 entries on the same long-history Agents; count remaining failures and record latency. Do not issue messages or cancel/replay Agents.
7. Track the runtime model-proxy transport warnings separately. These code changes are not a verified fix for the 502; a recurring transport error needs sanitized DNS/connect/TLS/timeout classification and authorized NyxID/edge evidence.

### Read-only query method

Use explicit context `gke_example-project-02_asia-northeast1_example-cluster-02`. GET endpoint paths (URL-encode query strings) are:

- VictoriaLogs: `/api/v1/namespaces/monitoring/services/vlsingle-vm-stack-victoria-metrics-k8s-stack:9428/proxy/select/logsql/query` with `query`, `limit`.
- VictoriaMetrics: `/api/v1/namespaces/monitoring/services/vmselect-vm-stack-victoria-metrics-k8s-stack:8481/proxy/select/0/prometheus/api/v1/query_range` with `query`, `start`, `end`, `step`.
- Metric inventory: same VictoriaMetrics prefix followed by `api/v1/label/__name__/values`. No `mongodb` series were present; only unrelated `node_memory_Cma*` names matched a broad mongo/CMA search.

Log scope: `cluster:"example-cluster-02" kubernetes.pod_namespace:"chrono-bot" kubernetes.container_name:"api" _time:[START,END]`. Add one fixed phrase: `"MongoDB client latency summary"`, `"Agent connection reconciliation completed"`, `"Agent work publication timing"`, `"Agent provenance backfill"`, or `"finished processing request"`. Parse only numeric fields and allowlisted route templates; process resource identities in memory for matched cohorts and retain no message bodies or raw request URLs.

CPU PromQL: `sum by (pod) (rate(container_cpu_usage_seconds_total{cluster="example-cluster-02",namespace="chrono-bot",pod=~"cma-agent-manager-.*",container="api",metrics_path="/metrics/cadvisor"}[1m]))`. Selecting only cAdvisor avoids duplicate resource metrics. Use 30-second samples for incident timelines and one-minute samples for four-hour comparisons.

## Existing evidence read first

- Lead's `2026-11-04-cma-latency/latency.md`, its aggregate evidence and complete-cohort coverage.
- `2026-11-04-cma-ux-regress/ux-regress.md` and `2026-11-04-cma-migrations.md`.
- Read-only `cma-502/cma-502.md` report for the runtime failure boundary.
- #1299 and #1301 issue comments, including retry publication/lease and tool deadline evidence.
- #1316/#1317/#1318 file inventories. This implementation also excludes `sandbox/funding.rs` and `store/sandbox_billing.rs`.
