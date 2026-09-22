# CMA Event Gateway operations

The wire authority is [CMAEG protocol](CMAEG_Protocol.md). Configuration is loaded
once before binding; invalid values fail startup. Restart through the drain
procedure to apply changes. Configuration is operator-owned.

## Configuration

| Environment key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `CMAEG_LISTEN` | socket address | `0.0.0.0:8090` | CMAEG listen address. |
| `CMAEG_METRICS_LISTEN` | socket address | `0.0.0.0:9091` | Separate Prometheus scrape listener; `/metrics` is absent from the public router. |
| `CMAEG_PROVIDERS` | JSON array | `[]` | Strict JSON provider allowlist described above. |
| `CMAEG_PROVIDER_CONNECT_TIMEOUT_MS` | positive integer, ms | `2000` | TCP/TLS connection timeout for each provider pool. |
| `CMAEG_PROVIDER_REQUEST_TIMEOUT_MS` | positive integer, ms | `30000` | Deadline for provider setup and probe requests, and receipt of streaming HTTP response headers. Active streams are owned and bounded by their consumer. |
| `CMAEG_PROVIDER_MAX_INFLIGHT` | positive integer | `256` | Deployment-wide upper bound applied to each provider's semaphore. Calls wait at most 50 ms for a provider permit before returning a typed busy result. |
| `CMAEG_PUBLIC_URL` | origin URL | Required | Externally reachable origin used in channel callback and tool URLs, without a trailing slash. HTTPS, or loopback HTTP for local tests. |
| `CMAEG_MASTER_KEYS_FILE` | file path | Required | Mounted file containing one `kid:base64key[,kid:base64key]` line; each key decodes to 32 bytes and the first key is current. |
| `CMAEG_CMA_BASE_URL` | origin URL | Required | CMA record-store origin. HTTPS, or loopback/`.svc` HTTP; no credentials, path, query, fragment, or trailing slash. |
| `CMAEG_CMA_SERVICE_TOKEN_FILE` | file path | Required | Mounted file containing the service-account bearer for CMA channel records and gateway rekey requests. |
| `CMAEG_NYXID_BASE_URL` | origin URL | Required | NyxID origin for creator authentication, with the same URL restrictions as the CMA origin. |
| `CMAEG_TOOLS_NYXID_SLUG` | string | `cmaeg` | NyxID service slug in the provider tools advertisement, matching `[a-z0-9_]{1,32}`. |
| `CMAEG_CHANNEL_CACHE_TTL_MS` | positive integer, ms | `60000` | Fresh lifetime of a resolved channel on this replica. |
| `CMAEG_CHANNEL_NEGATIVE_TTL_MS` | positive integer, ms | `10000` | Lifetime of a cached CMA record-not-found result; expired negative entries are never served. |
| `CMAEG_CHANNEL_CACHE_MAX` | positive integer | `50000` | Maximum positive and negative channel entries on this replica. |
| `CMAEG_RESTORE_RATE_PER_MINUTE` | positive integer | `10` | Per-channel restore refill rate, with an initial burst of three. |
| `CMAEG_RESTORE_TIMEOUT_MS` | positive integer, ms | `800` | Total cold-restore deadline, inside the record client's 1.5-second HTTP deadline. |
| `CMAEG_JWKS_TTL_MS` | positive integer, ms | `600000` | Fresh lifetime of relay signing keys; at most one refresh every five seconds per retained URL. |
| `CMAEG_INGRESS_MAX_BODY` | positive integer | `1048576` | Callback body limit, configurable from 1 byte through 16 MiB. |
| `CMAEG_INGRESS_INFLIGHT` | positive integer | `256` | Concurrent callbacks per replica; overflow returns immediate 429 with `Retry-After: 1`. |
| `CMAEG_ACCEPT_BUDGET_MS` | positive integer, ms | `20000` | Time from the last inbound body byte to acknowledgment; a pending provider request continues after the gateway returns 202. |
| `CMAEG_TURN_TIMEOUT_MS` | positive integer, ms | `600000` | Total turn lifetime including provider setup, retry and stream consumption. |
| `CMAEG_NYXID_REPLY_TIMEOUT_MS` | positive integer, ms | `5000` | NyxID reply connect/send/read timeout; uncertain sends are never blindly retried. |
| `CMAEG_ACK_TEXT` | string | `Working on it…` | Progress acknowledgement for long turns; nonempty text up to 1024 bytes without controls. |
| `CMAEG_HEARTBEAT_MS` | positive integer, ms | `60000` | Inactivity after acknowledgement before a heartbeat, capped at five per turn. |
| `CMAEG_FAILURE_TEXT` | string | `Sorry, I couldn’t finish that request. Please try again.` | Short failure notice; same bounds as acknowledgement text. |
| `CMAEG_DRAIN_SECONDS` | positive integer, seconds | `600` | Maximum shutdown drain for tracked turns and HTTP requests. |
| `CMAEG_BUSY_RETRY_MAX_MS` | positive integer, ms | `15000` | Maximum interval for retrying upstream busy refusals with jitter. |

Provider entry shapes and outbound policy are described in
[CMA operations](CMA_Operations.md#cmaeg). Public URL must have no trailing slash.
Master keys and the CMA service bearer are mounted files, never ConfigMap values.
Master key IDs are unsigned bytes; each base64 key decodes to 32 bytes, and the
first key is current. RUST_LOG controls the tracing filter through the standard
tracing subscriber. The latency harness has a separate test-only delay setting
documented below; it is not a deployment key.

## Observability

CMAEG serves Prometheus text at `/metrics` on `CMAEG_METRICS_LISTEN`
(`0.0.0.0:9091` by default). The public listener does not expose that path.
Keep the scrape listener private to the cluster. Registration is process-wide
and idempotent. All duration histograms use seconds with buckets
`0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10`.

| Metric | Labels | Measurement |
| --- | --- | --- |
| `cmaeg_ingress_to_provider_seconds` | `source`, `cold` | Last inbound body byte to first provider request socket write. The transport observation is required before recording a sample. |
| `cmaeg_restore_duration_seconds` | `outcome` | A shared restore flight, including the bounded CMA fetch, validation, and decryption. |
| `cmaeg_provider_accept_seconds` | `provider` | First socket write of the accepted attempt through the provider's first valid acceptance frame. |
| `cmaeg_turn_duration_seconds` | `provider`, `outcome` | Local turn consumer lifetime, including cancellation and timeout. |
| `cmaeg_delivery_seconds` | `platform`, `outcome` | A logical outbound message or custom terminal delivery, including route pacing and permitted retries. |
| `cmaeg_tool_call_seconds` | `tool`, `outcome` | Shared REST/MCP tool operation, including authorization and waiting for its receipt. |
| `cmaeg_events_total` | `source`, `outcome` | Ingress responses, distinguishing deliberate refusals from accepted and rejected requests. |
| `cmaeg_refusals_total` | `reason` | Bounded admission refusals and protocol error codes. |
| `cmaeg_provider_errors_total` | `provider`, `kind` | Classified turn failures; provider diagnostic strings are never labels. |
| `cmaeg_turns_inflight` | none | Admitted local consumers; decremented on completion or cancellation. |
| `cmaeg_channels_cached` | none | Locally retained positive and negative cache entries, including entries awaiting eviction. |

Sources, outcomes, tools, platforms, and error kinds come from finite gateway
classifications. Provider names come from the boot-time allowlist. Channel IDs,
event IDs, request IDs, credentials, and message contents are never metric labels.
`cold=true` belongs to the request that initiated the restore flight; concurrent
waiters and cache hits do not claim to have initiated it. A stale cache lookup
that starts a background refresh also triggers a restore.

Every public router generates a UUID v4 `x-request-id` when the caller did not
provide one, propagates it in the response, and creates one
`cmaeg.request{request_id,route}` span. The route is a template, so event references
in tool URLs do not enter the log. Child restore, verification, turn, and delivery
spans use a 16-character SHA-256 channel prefix and an event ID when available.
Turn and delivery tasks retain the originating trace across task and queue
boundaries. Info logs exclude bearer keys, reply tokens, event references, and
message content. Provider-controlled parsing diagnostics are also excluded.

Ingress responses include `Server-Timing` fields `restore`, `verify`, `launch`,
and `total`, in milliseconds. `total` includes body reading and waiting for the
acceptance response; it is at least every measured part. `launch` is an elapsed
time from body completion, so it overlaps the restore and verify parts and the
fields must not be summed. Without an observed provider socket write, it is
`launch;dur=0;desc="not_written"`; this includes early refusals and integration
compositions without an observed transport. Such values are not successful
zero-latency launches. REST and MCP tools responses include `deliver;dur`,
covering the complete request through the receipt or refusal.

The Responses transport observes successful TCP writes. HTTP/1 connections are
pooled with up to 64 idle connections per provider and a 90-second idle lifetime.
HTTP/2 requests share a connection; cleartext `.svc` providers use prior knowledge,
and HTTPS providers negotiate HTTP/2 or HTTP/1.1 through ALPN. An HTTP/2 frame's
stream ID associates its first HEADERS byte with the right request; SETTINGS,
PING, and other control frames cannot produce a launch sample. The observer reads
only the fixed nine-byte frame header and never decodes credentials or content.

TLS timing is taken below encryption, at the first TCP write of the TLS record
containing the request's first plaintext segment. Earlier handshake and control
records are flushed before that segment is armed. Accepting bytes into a TLS
buffer does not record a write. Provider headers, acceptance, and response content
arrive afterward and do not affect the ingress sample. Retries retain the first
attempt's ingress sample; acceptance duration refers to the accepted attempt.
If acceptance times out before the write, the already-returned header cannot be
updated; the metric records the later write if the admitted turn reaches it.

Each turn's semaphore remains held for the stream lifetime. Dropping an HTTP/2
turn resets its stream while other turns keep their connection. HTTP/1 terminal
frames permit a bounded 100-ms, 16-KiB tail drain for connection reuse; cancelled
consumers close immediately. Request header deadlines remain independent of
stream lifetime. Normal certificate verification remains enabled. The transport
uses the workspace's existing locked Hyper, h2, Tokio, and Rustls dependency
versions; it does not add an alternative TLS stack.

Run the executable latency gate from `api/`:

```sh
cargo nextest run -p cmaeg --run-ignored ignored-only -E 'test(latency_budget)' --locked --success-output final
```

The ignored integration test composes the real management, ingress, turn runner,
tools, and delivery services with local fake CMA, JWKS, provider, and NyxID
endpoints. It creates one channel through the management API, then relocates the
JWKS URL inside the test backend's encrypted record to the local fixture. The
production signature, issuer, restoration, authorization, and provider IO paths
remain active. RSA signing keys are generated at runtime.

The warm run sends 500 callbacks with 16 concurrent callers. Its p50 must stay
below 50 ms and p99 at or below 1000 ms. It asserts there are no CMA reads or
additional JWKS fetches. The cold run sends 200 callbacks in 25 rounds of eight;
each caller starts after cache invalidation, and the eight callers share one
restore flight. It asserts 25 additional CMA reads, no additional JWKS fetches,
launch p99 at most 2000 ms, and restore wait p99 at most 800 ms. The initiating
caller owns the `cold=true` metric sample; all eight wait on the cold cache.
Provider processing time is outside both launch budgets.

The fake CMA read delay defaults to 50 ms and is configurable through the
test-only `CMAEG_LATENCY_RESTORE_DELAY_MS` environment variable. Forced repeated
evictions use an elevated configured restore rate so the harness measures the
restore path; production defaults remain unchanged. The channel uses explicit
tool replies and disables automatic replies and acknowledgements. The provider
still streams a message and completes every turn.

The tool run sends 200 authenticated `replyToEvent` requests through the real
delivery engine to the fake NyxID receiver. Sixteen configured routes run in
rounds; the next round begins three seconds after the previous receipts. The
production route pacing is active, every receipt must confirm delivery, and the
receiver independently checks each route's send interval. Tool `deliver` p99
must stay at or below 500 ms. This measures the tool request on an available
route; requests queued behind the same route's previous send include their
pacing wait and can take longer.

The test prints nearest-rank p50/p90/p99 and writes `api/target/cmaeg-latency.txt`.
CI's `cmaeg-latency` job runs after `backend`, has a fifteen-minute limit, retries
the complete harness once on failure, and uploads that report even on failure
when it exists. The second failure fails the job. No thresholds are relaxed in
CI. The workflow is committed locally; its hosted result requires a future push.

Local evidence on macOS ARM64, 2026-09-22, with the default 50-ms fake CMA delay:

| Measurement | Samples | p50 ms | p90 ms | p99 ms | p99 limit ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Warm launch | 500 | 1.241 | 1.479 | 3.029 | 1000 |
| Cold launch | 200 | 48.937 | 57.036 | 59.157 | 2000 |
| Cold restore wait | 200 | 47.174 | 55.824 | 57.984 | 800 |
| Tool delivery | 200 | 1.887 | 3.807 | 8.678 | 500 |

That run also scraped the private metrics listener with `curl` after the measured
requests and verified 200 confirmed Telegram deliveries, one cached channel,
and zero remaining local turns. The scrape is written to
`api/target/cmaeg-metrics.txt`. These are local fixture measurements; they do not
claim deployed provider or platform latency.

## Deployment and runbooks

Run the gateway as a separate stateless workload with its public listener and
private metrics listener on separate service ports. Readiness is `/readyz`;
liveness is `/healthz`. Readiness gates loaded master keys, parsed provider policy and draining state.
Its CMA reachability field is diagnostic only; remote outages do not remove a
warm replica from service. A public route must never forward the private scrape port.
`infra/cmaeg/Dockerfile` builds the gateway image; the dedicated deployment issue
adds Kubernetes manifests. No deployment is performed by the protocol checks.

Mount the master-key ring and CMA service token read-only. Grant the service token
only the CMA channel-record and gateway management authority it needs. Register
the tools service under `CMAEG_TOOLS_NYXID_SLUG` with the four protocol operation
IDs. Configure provider paths explicitly and retain provider/binding ownership.
A channel's bot agent key supplies provider and tool authority; a relay user token
is never used as a provider credential.

For shutdown, remove the instance from new ingress through readiness, send SIGTERM
or SIGINT, and allow at least `CMAEG_DRAIN_SECONDS` plus load-balancer propagation
and process-exit margin before force termination. The gateway first drains
admitted turns and then queued delivery under a single deadline. Ephemeral output
can be lost if the deadline expires. A rollout does not stop provider Sandboxes;
the provider remains responsible for their durable lifecycle and admission.

For a restore failure, inspect `cmaeg_restore_duration_seconds` outcomes and
`cmaeg_refusals_total`, then check CMA reachability, the service-token mount,
record ownership, and the retained key IDs. Repeated misses can consume the
per-channel restore burst; wait for its refill instead of raising the global
limits first. Invalid records fail closed. Positive stale entries can be served
while a bounded refresh runs; negative entries expire strictly. Deletion or key
revocation can therefore take a cache lifetime to reach another replica.

For latency above budget, compare warm/cold ingress histograms, restore durations,
provider acceptance and delivery durations. High acceptance with low launch is
provider work. High cold launch and restore is record retrieval/decryption. High
same-route delivery can be the required three-second Telegram pacing. Verify the
private metrics scrape directly; a `not_written` launch header is not a fast
successful request. Run the local harness before changing timeouts and retain
its latency and metrics artifacts with the investigation.

For authentication refusals, identify the bounded problem/refusal code and request
ID. Check issuer, route membership, signing-key rotation, callback timestamp and
exact body bytes; do not paste tokens, sealed references or content into logs.
A JWT `kid` never changes the pinned JWKS URL. Creator auth caches positive results
for 60 seconds and failures for ten seconds. Tool authentication is local against
the retained key hash; rotate it through channel update and allow cache expiry on
other replicas. `channel_key` and `agent_key` have different roles.

For master-key rotation, add the new key first and retain old keys on all replicas
before making it current. Invoke the service-authenticated rekey operation for
known channel IDs, checking optimistic conflicts. Existing channel IDs still
contain pointers sealed by their original key, so rekeying only the record does
not make that old pointer key disposable. Keep it while those channel IDs remain
valid. Never remove a key based only on a successful rekey count. Back up the ring
through the secret-management system; the encrypted CMA record alone is not a
recoverable credential backup.

For provider busy failures, inspect inflight turns and upstream `Retry-After`.
Retries keep the event idempotency key. Do not synthesize a new event ID to force
admission; that can create another turn. For uncertain platform delivery, inspect
the receipt's `unknown` outcome and the sink, then reconcile manually if needed.
The gateway does not blindly resend unknown Telegram sends. Webhook receivers
must deduplicate stable webhook IDs across the four bounded attempts.
