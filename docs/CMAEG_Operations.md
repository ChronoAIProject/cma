# CMA Event Gateway operations

## Public documentation

The gateway itself serves a single `/docs` document combining the integration
guide, architecture and complete protocol, with section navigation and a
`/docs/download` Markdown download. The former `/docs/architecture` and
`/docs/protocol` addresses redirect to their sections. `/openapi.json` remains
the machine-readable contract. Individual source articles are also available
verbatim at `/docs/integration.md`, `/docs/architecture.md` and
`/docs/protocol.md`. They require no login, JavaScript or external assets.
The build embeds exactly the reviewed integration, architecture and protocol
Markdown files; operations runbooks are not part of the public bundle.
`infra/cmaeg/Dockerfile` copies those inputs for the binary build. Documentation
updates therefore roll out with the gateway image, independently of CMA web.

Service flow diagrams are reviewed static SVG assets under
`api/crates/cmaeg/assets/docs`. Regenerate them with
`.github/scripts/render-cmaeg-diagrams.py` using Mermaid CLI 11.17.0 after editing
Mermaid source blocks; `--check` verifies source freshness in CI. The browser
loads local SVG images under the gateway's restrictive content security policy.
The complete Markdown download retains the Mermaid sources and internal links.

Production catalog display name: **Agent Event Gateway**. NyxID slug: `cmaeg`.
The catalog REST base is `https://bot-gateway.chrono-ai.fun/v1`, with OpenAPI at
`https://bot-gateway.chrono-ai.fun/openapi.json`. The `/v1` base is necessary
because the NyxID OpenAPI parser discovers paths but does not apply `servers`.
The public/internal service is auto-connected and forwards caller bearers;
channel ownership and credential-specific gateway checks remain authoritative.

## Startup

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
| `CMAEG_MASTER_KEYS` | secret string | Required unless file source is set | Kubernetes Secret environment value containing `kid:base64key[,kid:base64key]`; each key decodes to 32 bytes and the first key is current. |
| `CMAEG_MASTER_KEYS_FILE` | file path | Optional alternative | Compatibility file source for the same key ring; mutually exclusive with `CMAEG_MASTER_KEYS`. |
| `CMAEG_CMA_BASE_URL` | origin URL | Required | CMA record-store origin. HTTPS, or loopback/`.svc` HTTP; no credentials, path, query, fragment, or trailing slash. |
| `CMAEG_CMA_SERVICE_TOKEN` | secret string | Required unless file source is set | Kubernetes Secret environment value authenticating inbound gateway rekey. Also used for outbound CMA authentication when the service-account client pair is absent. |
| `CMAEG_CMA_CLIENT_ID` | string | Optional pair | Dedicated NyxID service-account client ID, injected from Kubernetes Secret. Must be set together with its client secret. |
| `CMAEG_CMA_CLIENT_SECRET` | secret string | Optional pair | Exchanges renewable outbound CMA tokens at the configured NyxID origin using `client_credentials` and `openid roles`. The account role grants `cma:gateway:records` and `cma:provider:bindings`. |
| `CMAEG_CMA_SERVICE_TOKEN_FILE` | file path | Optional alternative | Compatibility file source for the same bearer; mutually exclusive with `CMAEG_CMA_SERVICE_TOKEN`. |
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
| `CMAEG_BUSY_RETRY_MAX_MS` | positive integer, ms | `180000` | Maximum interval for retrying upstream busy refusals, including first-conversation preparation. Jitter is added after the provider's `Retry-After` minimum. |

Provider entry shapes and outbound policy are described in
[CMA operations](CMA_Operations.md#cmaeg). Public URL must have no trailing slash.
Master keys and the CMA service bearer are Kubernetes Secret environment values,
never ConfigMap values.
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

The `request finished` info record includes the exact `server_timing` header
for ingress and tool routes. A separate `ingress socket write` info record emits
`ingress_to_provider_ms`, `source`, and `cold` once per observed first provider
write, under the originating `cmaeg.request` span. The value matches the histogram
sample, before bucket aggregation. It can arrive after the HTTP response if the
acceptance budget elapsed before the write; retain that record instead of treating
the earlier `not_written` header as a zero-latency sample. Repeated transport
notifications and retries do not create extra first-write samples.

For a live latency campaign, retain raw metric scrapes and these timing records
with the pod UID/image, request IDs and measurement window. Calculate nearest-rank
percentiles from the per-write values, separating `cold=true` initiators from
warm requests. Histogram buckets provide threshold bounds rather than exact
per-message percentiles. Metrics have no channel label: an isolated test window
or request correlation is required before attributing aggregate counts to one Bot.

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
CI. Hosted results are recorded on the corresponding pull request's Actions run.

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
`infra/cmaeg/` declares a separate `cmaeg` Deployment with **one replica** in
`chrono-bot`, matching the maintainer's 2026-09-22 deployment decision. CMA API
also keeps one replica. The gateway listens on 8090; its private metrics listener
uses 9091 and is not exposed through the public Service. The HTTPRoute attaches
to `gateway-infra/chronoai-gateway` at `bot-gateway.chrono-ai.fun`; public DNS and
certificate coverage remain cloud-side prerequisites. The single-label hostname
fits the existing wildcard certificate.

Use native Kubernetes Secrets, injected as environment variables through explicit
`secretKeyRef` entries. `cmaeg-master-keys` supplies `CMAEG_MASTER_KEYS`, and
`cmaeg-cma-service-token` supplies `CMAEG_CMA_SERVICE_TOKEN`,
`CMAEG_CMA_CLIENT_ID`, and `CMAEG_CMA_CLIENT_SECRET`. Populate both Secrets
out of band. `secret.yaml` contains names and key contracts only, never values.
There are no credential file mounts, CSI resources, or Google Secret Manager
dependencies for the gateway. Its service account has no RBAC grants and does not
mount a Kubernetes API token. Existing `_FILE` inputs remain optional for other
installations; setting both sources for a credential fails startup.

Secrets are read at process startup. Preserve the key ring across pod replacements.
Production supplies the service-account client pair because NyxID access tokens
expire after one hour. CMAEG obtains a token before opening its listener and
shares its cache across records and provider management. Requests renew the token
within the final minute of its lifetime; concurrent callers share one exchange.
A failed refresh can use the existing token only until expiry, and retries are
bounded. Exchange responses and credentials are never logged, and redirects are
refused. Renewal requires no Secret update, file, pod restart, or extra workload.

`CMAEG_CMA_SERVICE_TOKEN` remains a separate stable inbound rekey credential when
renewable authentication is enabled. It is not sent to CMA in that mode. Static
outbound bearer configuration remains available for other installations; those
operators must renew it and restart before expiry. Client-secret rotation also
requires a Secret update and a gateway rollout. Retain the same service-account
subject so stored CMA records and provider bindings remain accessible. Grant the
account only the two gateway permissions. CMA uses the advertised native `/mcp`
endpoint with a runtime-local credential proxy; no NyxID tools catalogue entry
or owner connection is needed. `CMAEG_TOOLS_NYXID_SLUG` remains in the provider
advertisement for providers that use catalogue integration. Configure provider
paths explicitly and retain provider/binding ownership.
A channel's bot agent key supplies provider and tool authority; a relay user token
is never used as a provider credential.

The deployment permits a rolling replacement with one surge pod and zero
unavailable pods. Its 615-second termination grace covers the five-second
load-balancer propagation delay, the 600-second application drain, and teardown.
Apply ConfigMap and populated Secrets before starting the Deployment. Confirm
`/healthz`, `/readyz`, accepted HTTPRoute/health policy status, authenticated CMA
record access, and the public HTTPS route after rollout. A ready gateway alone
does not prove a deployed Telegram round trip.

For shutdown, remove the instance from new ingress through readiness, send SIGTERM
or SIGINT, and allow at least `CMAEG_DRAIN_SECONDS` plus load-balancer propagation
and process-exit margin before force termination. The gateway first drains
admitted turns and then queued delivery under a single deadline. Ephemeral output
can be lost if the deadline expires. A rollout does not stop provider Sandboxes;
the provider remains responsible for their durable lifecycle and admission.

For a restore failure, inspect `cmaeg_restore_duration_seconds` outcomes and
`cmaeg_refusals_total`, then check CMA reachability, the injected service token,
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

### Recovering channel creation

A durable creator can reserve a UUID, save it with its own operation, and include
it as `record_id` in `POST /v1/channels`. CMAEG writes the encrypted pending
record before calling the provider. Recover a lost response through
`GET /v1/channel-records/{record_id}` with the same owner's bearer. The response
retains the original opaque channel ID, callback URL, binding ID, and record
version. `definition.creation.ready=false` means provider binding has not been
committed; event ingress and policy updates remain unavailable. Retry the exact
create body or delete the recovered channel with its current version. A creator
that has discarded transient credentials must recover and clean up the pending
record, then start a new operation with a fresh UUID. Never reuse a retired UUID
or run cleanup concurrently with an outstanding create.

Local fake-service tests cover recovery after gateway restart, exact identities
across replicas, changed-content conflicts, owner concealment, and retained
cleanup identity after provider refusal. These tests do not establish deployed
provider or Telegram evidence.

### Production deployment on 2026-09-22

CMA and CMAEG were deployed to namespace `chrono-bot` in
`gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke`, with one
ready replica each. The rollout was verified at 11:50 UTC. PR #922 merged into
`develop` at `f297f7c98ad9cfa903009eea37089c892b66283f`, closing #917.

Both native `linux/amd64` images came from successful Actions run `35717639418`,
attempt 1. Their actual source revision is
`1cbd86a67d24a3f7ab3b510010fd54f4ac60af94`, with tree
`2529cf609a48d9f07c045e8c15bce7ed85ce238e`, independently verified equal to the
merged `develop` tree. Archive sizes/checksums, immutable Docker image IDs,
architecture, labels, OCI layer digests and semantic configuration equality
were checked before transfer. Registry manifest digests and configurations were
read back after publication; source labels retain the tested revision.

The repository prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Verified deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:72a2f622e50dbc2f72afbb2e1e00a9380b7866e324eb3cfd38bacfc211be4d78` |
| CMAEG | `cma-cmaeg` | `sha256:3a573e6f1d1ee6f21f73a5ec573119e9936db693988c2b354d6afa172dc05f5a` |

All 14 CI jobs passed, including 2,896 regular backend tests, 668 MongoDB
persistence tests, all four image builds/scans and the gateway latency harness.
The harness measured warm launch p99 23.374 ms, cold launch p99 65.347 ms,
cold restore p99 55.969 ms and tool delivery p99 18.211 ms. Those measurements
use the local fake services in CI; they are not deployed Telegram measurements.

The API image contains the production bridge fixes through `2917b8a3` together
with the gateway/provider implementation. The live rollout changed only its
container image and preserved all configuration, enabled bridge flags and the
runtime default `bed09cb7`. A fresh nine-Sandbox baseline taken immediately
before rollout retained all nine Pod UIDs, container images, readiness and
restart counts afterward. This is Pod continuity evidence, not an in-flight
Telegram restart drill.

The approved non-default NyxID role `cmaeg-service` and service account
`cmaeg-prod` were provisioned with exactly `cma:gateway:records` and
`cma:provider:bindings`. Native Kubernetes Secrets supply the stable master key
ring, client credentials and a separate stable inbound rekey bearer. Values
were generated or received in memory and sent directly to Kubernetes; no
gateway credential files were written. The measured outbound token lifetime is
3,600 seconds, and this image includes automatic client-credentials renewal.
Startup against real NyxID and the compiled binary's expiry/renewal journey
passed. No existing human NyxID roles were changed. The earlier role-inventory
403 was Cloudflare error 1010 rejecting a generic Python client signature;
the administrator's account has the required authority.

`bot-gateway.chrono-ai.fun` previously had no dedicated DNS record and inherited
a wildcard pointing at an older ingress. A proxied Cloudflare A record now
points to the shared production gateway at `136.69.78.214`, matching the API's
origin. The existing certificate covers the hostname. Both HTTPRoutes are
accepted with resolved references; the backend and health policies are attached.
Direct HTTPS and public HTTPS through Cloudflare both return healthy readiness,
and HTTP redirects to HTTPS. The private metrics listener returns HTTP 200
through a local port-forward; `/metrics` on the public listener returns 404.

Public read-only smoke checks verified CMA and CMAEG readiness, CMA's exact two
service-account capabilities, authenticated record listing, typed missing-binding
and missing-channel responses, and HTTP 401 on unauthenticated record reads.
The missing-channel request exercised the deployed gateway's renewable service
credential against CMA without creating user resources. #918 remains open:
Telegram round trips, deployed latency measurements, failure notices and the
conversation restart drill have not been performed by this rollout.

### Production Bot integration rollout on 2026-09-22

The production CMA API and CMAEG were updated in `chrono-bot`, with one ready
replica each. Backend Bot integration PR #924 merged into `develop`, and PR #926
records the deployed image pins. Both public readiness endpoints return HTTP 200;
CMAEG reports `cma_reachable: true`.

The registry prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:34450d1e649a38f06239f721a082ac26c43d3ae50aa2446aae4d7c47d02100ca` |
| CMAEG | `cma-cmaeg` | `sha256:e6b41876feeb2e1857a44600a1fab4576154a80c902e13a3ef9a78e09c74b0e0` |

The images came from successful CI run `35743231350`, tested revision
`72f42ffce333dcc67fd99df6fc0ee8422615e2b7`. The tested tree was verified equal
to the merged PR #924 tree before deployment. Archive checksums, image
configuration and labels, OCI blobs, and registry digests were verified.
Native Kubernetes Secrets continue to supply gateway credentials.

The API now has `CHRONO_CMAEG_BASE_URL=http://cmaeg` and
`CHRONO_CMAEG_PUBLIC_URL=https://bot-gateway.chrono-ai.fun`. All nine existing
Sandbox Pod UIDs and image pins were preserved. A before/after comparison for
seven owner Agents passed for public lineage, profile pins, durable event
prefixes, and runtime Pod/container identity. It does not establish continuous
availability between samples, native conversation identity, dirty-file
preservation, duplicate-input absence, browser drafts/reconnect, or cleanup
safety. A subsequent read-only public API inventory found one operator-visible
Bridge in the `enabled` state. The production configuration still enables
management, ingress, allocation, execution, outbound and Telegram bridge
switches. This owner-scoped inventory is not a global bridge-absence check.

Read-only verification at 2026-09-22 17:24 UTC found no `cmaeg` catalogue service
or owner connection in NyxID. The maintainer authorized the existing personal
Telegram bot for testing and directed that NyxID configuration and deployment
remain unchanged. The current CMA Bot flow requires that service connection,
and CMA's provider reports `tools.native_mcp_url` as unsupported. Therefore the
real Telegram tools, latency, restart and failure campaign has not run, and #918
remains open. No bridge migration or removal is established by this rollout.

At this rollout, the Bot dashboard and standalone gateway SDKs were still
awaiting merge. This deployment record does not establish their production
acceptance.

### Production timing evidence rollout on 2026-09-22

At 18:07 UTC, CMAEG was updated to
`cma-cmaeg@sha256:952e60116d5db599e7516e48bcd48d9762c87078b3a6ba8f94353978dfa6a25f`
under the same registry prefix. The Deployment has one desired, ready and
available replica. The nine existing Sandbox Pod UIDs, container identities,
images and restart counts were unchanged across the rollout.

All 14 jobs in CI run `35759943816` passed, including real MongoDB persistence
contracts and the latency gate with the production logger enabled. Its native
gateway image has source revision `1fd368a43fa353b1de1740e79d6d1fd9cb06f546`,
whose tree `571b515ee746340c2b5a2cdb4a95452eefe3d63f` equals PR #929's merged
`develop` tree at `dd5f84ead36a9693928e8bcd4f5734db387686df`. Archive, image
configuration, labels and OCI blobs were checked, and the registry manifest and
configuration were read back before applying the conditional image patch.

The CI fixture measured warm launch p99 26.225 ms, cold launch p99 61.616 ms,
cold restore p99 54.632 ms and tool delivery p99 13.060 ms. These are fixture
measurements, not production Telegram evidence. Both public readiness checks
passed after deployment. An unauthenticated tool-context request returned 401,
and its `Server-Timing` header matched the production request-completion log
under the supplied request ID. This confirms deployed response timing logging;
it does not establish a provider-write sample or Telegram delivery. No NyxID
configuration or deployment changed, and no legacy bridge was migrated or
removed by this rollout.


### Production Bot dashboard rollout on 2026-09-22

PR #928 merged into `develop` as `5dbe75d0`. All 14 jobs in CI run
`35765699875` passed, including the gateway latency gate. The tested revision
`ea917d215d2f5cffdaba6f8680ced0253877e0c0` and merged commit share tree
`9d3d48af2fdc1effcd882dfc19659272f75f74fe`. The CI archive, source labels,
OCI blobs, manifest and registry configuration were verified before deployment.

At 18:58 UTC, `cma-web` was ready on the deployed image
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-web@sha256:d79f5c2331a4bb3d401a33f1e44d06d30f58b6ca034d3de3cdf811ce83b39ef4`.
Only the web container image was patched, using deployment UID, resource version,
replica count and prior-image preconditions. CMA API, CMAEG and web each have
one ready replica in production `chrono-bot`. Public API/gateway readiness and
web HTML checks passed. All nine Sandbox Pod UIDs, container IDs, restart
counts, readiness and runtime images matched the fresh pre-rollout baseline;
the runtime default also remained unchanged.

This deploys Bot creation, list, status and lifecycle controls and closes #916.
It does not establish the real Telegram acceptance campaign in #918. Native
provider tool attachment is being implemented in CMA to respect the maintainer's
instruction to leave NyxID unchanged; that path is not enabled by this web rollout.
The original bridge is still retained pending migration and acceptance evidence.

### Native CMA tools rollout on 2026-09-22

PR #931 merged into `develop` as
`37394165a4df0a5007a6e53a23efa7a8ea431068`, including every commit from #930.
All fourteen required jobs in CI run `35786266601` passed. Its tested revision
`6c2e412b44c608b582844588b942ed42c04bb5cb` and the merged commit share tree
`13cedb931a48622f579764d8706e5b492e0d90da`. Archive checksums, native image
configuration, source labels and immutable registry references were verified.
The runtime image passed its executed Linux checkpoint contract and the pinned
Codex/ACP native-tool contract. No release workflow or `main` merge was used.

At 22:17 UTC, API and web rollouts and public readiness checks had passed in
production `chrono-bot`. API, CMAEG and web each have one ready replica. The
registry prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:d41b02c608f0764856e893ba8f41771c8f9e7f0c199625e9bf1f9919523340c4` |
| CMA web | `cma-web` | `sha256:99985942a90578f0351ed51642c394bc230c324be3556f9cc08548a52810a4c3` |
| New-allocation runtime default | `cma-runtime` | `sha256:bf7fe30a8259015115beb81e8a8d6fbb4394d85e83dcef67e30ca8637e735905` |
| CMAEG, retained from its timing rollout | `cma-cmaeg` | `sha256:952e60116d5db599e7516e48bcd48d9762c87078b3a6ba8f94353978dfa6a25f` |

Server dry-runs preceded all writes. Deployment UID, resource version, replica
count and prior-image preconditions fenced the API/web patches. The runtime
default ConfigMap and API annotation changed together. Native Kubernetes
Secrets continue to supply gateway credentials. NyxID catalogue, configuration,
code and deployment were unchanged.

The initial inventory contained nine runtime Pods. The first broad comparison
correctly detected three replacements and paused acceptance work. An exact-ID,
majority-read Mongo audit then classified six as user-owned Sandboxes and three
as unowned warm reservations. All six owned Pods preserved their UID, Pod spec,
container ID, image digest, readiness and restart count. The three unowned
reservations were retired by the existing warm-pool controller and replaced
under the new default; none had an owned Sandbox record. The audit's temporary
Pod was removed with a UID precondition and absence was verified.

Public observations at 22:12:44 and 22:14:22 UTC passed for seven selected Agents:
lineage, profile pins, durable event prefixes and the associated runtime identity.
This does not establish continuous availability between samples, native
conversation identity, dirty-file preservation, duplicate-input absence, browser
drafts/reconnect or cleanup safety.

CI's fake-provider latency run recorded warm launch p99 14.038 ms (500 samples),
cold launch p99 65.830 ms (200), cold restore p99 57.369 ms (200), and tool delivery
p99 17.124 ms (200). These are CI results, not production Telegram latency.

This deploys native CMAEG tool attachment and optional owner-selected Bot
environment profiles. After admitting either new record shape, this verified API
is the minimum compatible rollback floor; the former production API cannot read
all retained fields. Existing owned Sandboxes keep their original runtime pins.
Real custom-event and Telegram acceptance, including restart and failure drills,
remains under #918. The legacy bridge remains enabled and wired pending that
evidence and the retirement work in #920.

The first disposable custom event at 22:19:38 UTC exposed a first-conversation
readiness mismatch: Responses returned 404, the gateway successfully reserved
the conversation with PUT 201, and the immediate retry returned 409 while its
Agent was still provisioning. CMAEG correctly treated that unrecognized conflict
as a refusal and returned 503. No turn was admitted and no native-tool success
is claimed from this attempt. The existing Telegram route was unchanged.

CMA now returns `429 conversation_not_ready` with `Retry-After: 1` while the
reserved conversation is provisioning. This uses CMAEG's existing bounded busy
retry and retains the event body and idempotency key. Local regressions reproduce
the former 409 refusal, require retryable preparation, and verify one response
identity after readiness and replay. Gateway coverage also exercises a missing
conversation followed by busy preparation and eventual acceptance with one PUT
and unchanged POST bodies/keys. This follow-up still needs its own successful
CI, rollout and fresh live acceptance; it is not part of the image record above.

A second disposable custom channel at 22:36 UTC reproduced the refusal. An
independent attempt to inspect its prepared conversation found the Agent had
failed with `agent_runtime_rejected` before any turn. The native-tool authority
check required a Ready Worktree even though Agent startup deliberately keeps its
Worktree Creating until the conversation opens. The original Mongo fixture had
advanced the Worktree too early and therefore missed this integration mismatch.
The corrected fixture retains the real startup state and tests the first tool
attachment as well as retirement and authority fences. Creating Worktrees may
attach tools only while their Starting Agent opens its conversation or recovers
the pending ready publication; other preparation and removal states still refuse.

Neither disposable attempt establishes native-tool, full-context, restart or
Telegram acceptance. Both channels and their temporary keys were removed; the
second test's runtime teardown was verified at 22:42 UTC. The existing Telegram
route and original Sandbox were unchanged.


### First-conversation production follow-up — 2026-09-22

PR #932 merged into `develop` as
`328093cb1e2a496a1662c9b13bf5582bf1d06381` at 23:40 UTC after all 14 jobs in CI
`35794408644` passed. The merged tree exactly matches the tested merge revision
`f302ece0b83291abdbab6ed4d68d4833a9e532d4`. Backend recorded 2,961 passing tests;
the separate disposable Mongo contracts recorded 630 passes, including native
tool attachment with the real Creating Worktree startup state. The Linux writer
boundary, image gates, SDK/frontend checks and latency gate passed too.

The verified API archive was copied to the existing production registry and its
image configuration was compared again by immutable digest. At 23:43 UTC the
API-only rollout passed readiness with
`cma-api@sha256:a14cec2c7d63bd1bf8150774bed6ce6d5484bab57b4c14d3cd34ea24d73c4875`
under the registry prefix in the prior record. UID, resource-version, replica
count and prior-image checks fenced the server dry-run and update. CMA and CMAEG
remain at one replica each in `chrono-bot`. The web/gateway deployments, runtime
default and API configuration did not change; gateway credentials remain native
Kubernetes Secrets. NyxID configuration, catalogue, code and deployment did not
change.

All nine observed runtime Pods preserved their UID, full spec hash, container ID,
image digest, readiness and restart count across this API update. Public
observations at 23:41:05 and 23:43:14 UTC passed for seven selected Agents' lineage,
profile pins and durable event prefixes. The same observation limits documented
above apply; these snapshots do not prove continuous availability or dirty-file,
native-conversation and browser-reconnect continuity.

CI's fake-provider latency gate recorded warm launch p99 22.470 ms (500 samples),
cold launch p99 62.301 ms (200), cold restore p99 55.650 ms (200) and tool delivery
p99 15.253 ms (200). These remain CI measurements, not Telegram acceptance.
The original bridge remains enabled pending the live acceptance and retirement
prerequisites in #918 and #920.
The first follow-up cold custom event at 23:45 UTC progressed past both prior
startup defects, reached a Ready Agent, then ended with provider HTTP 409 and
gateway `503 provider_unavailable`. The provider problem code was not retained,
so its exact cause is unproved. A separate warm event admitted one turn and read
the full context, but guarded tool approvals and a failed reply call meant it
was not cold-path or reply acceptance. Its channel and temporary key were
removed, and its runtime Pod's absence was verified.

### Native custom context and gateway replacement — 2026-09-23

A fresh disposable channel used the intended autonomous permission profile and
sent one signed custom CloudEvent with exactly 32,768 bytes of text. Cold
conversation preparation retried with the same event and admitted a turn without
manual preparation. The stream completed with exactly one `cmaeg.message`
containing the expected suffix and no failed terminal frame. The public Agent
log recorded one started/completed turn, successful native `readEventContext`
and `sendTyping` calls, and a successful `replyToEvent` after one failed tool
attempt. This does not establish why the first tool attempt failed.

At 00:03:39 UTC, the authenticated CMA context endpoint and the gateway MCP tool
returned the complete text unchanged, with all four exact tool names and an
879-byte sealed reference. The idle gateway Pod was then replaced using its exact
UID and normal termination grace. The replacement kept the same immutable image
and deployment spec. All ten observed runtime Pods, including the disposable
Agent, retained exact UID, full spec hash, container ID, image, restart count and
readiness; API configuration and all deployment specs were unchanged.

At 00:05:54 UTC, the new gateway process returned exactly the same full context:
SHA-256 `26ee6c7b132630f2a8d05ca131f967584eacf57d024629daf93fe6f23c0b6b19`.
The before/after digest covers the complete returned context, not just text.
This proves persisted custom-event context survives gateway replacement. It is
not Telegram ingress, Telegram delivery, or the required latency campaign.
The existing Telegram route and original bridge remain unchanged.


### Admission rollout and restored Telegram migration — 2026-09-23

PR #933 merged into `develop` as
`1e6dd9ec1fda1d58ea55eb39ce01269026e9347f` after all 14 jobs in CI
`35802834774` passed. The merged tree matches tested revision
`9701a51ac982a3db6eb576d2a94c896cef6da66e`. Validation recorded 2,965 regular
backend tests and 669 real Mongo contracts, plus the Linux writer boundary,
built runtime execution/cleanup proof, image and SDK/frontend gates. The runtime
fix skips vanished processes on ENOENT or ESRCH during writer inspection; other
inspection errors still refuse the fence.

At 01:33:49 UTC, the verified API and future-runtime-default rollout passed with
`cma-api@sha256:aaf51a96e0720aa98b5789cd6194d392459308dfab0cfe1fd5118e85c3cddee2`
and
`cma-runtime@sha256:532a64cf193957991248d98d82b7a61f6d333a2b87f5e1db0a19ae55f598ed49`
under the existing production registry prefix. All six owned runtime Pods
retained exact UID, full spec, container identity, image, restart count and
readiness. Three unowned warm reservations retired and were replaced, with
majority Mongo ownership checks before and after. Only the runtime-image default
changed in API configuration. API, web and gateway each had one ready replica.

Seven selected Agents passed lineage, profile and durable history-prefix
comparisons. The first public continuity read after rollout returned HTTP 503;
a later observation succeeded. These snapshots do not establish uninterrupted
availability or explain that first failure, and retain the earlier dirty-file,
native-conversation, duplicate-input and browser-reconnect limitations.
CI fake-provider p99 timings were warm launch 13.724 ms, cold launch 59.088 ms,
cold restore 53.679 ms and tool delivery 9.862 ms; these are not Telegram timings.

The authorized dashboard migration then failed during `mint_agent_key` at
01:35:41 UTC. The failed Bot completed durable cleanup by 01:37:43 UTC with no
channel or pending operation; a subsequent exact-namespace key inventory was
empty. This does not prove key creation itself failed, because cleanup may have
revoked a minted key. CMA's adapter rejects NyxID's offset-form expiry metadata;
inspection also found PATCH requests where the existing key and conversation
update contracts require PUT. These are CMA client compatibility defects.

The original Telegram route was restored active at 01:36:46 UTC with its original
key and fields and no competing active route. The bridge was re-enabled at
01:36:57 UTC, revision 7. The original Sandbox, Workspace, Worktree and Agent were
preserved. No Telegram test message was sent, and no new gateway channel was
created. NyxID catalogue, service configuration, roles, code and deployment were
unchanged. Telegram acceptance remains incomplete, although GitHub issue #918
was closed separately at 01:28:36 UTC without recorded Telegram evidence. Bridge
retirement in #920 remains open and gated on that evidence.

A subsequent disposable live key check reproduced a further CMA contract
mismatch: NyxID rejects API-key `platform: telegram`; this metadata identifies
the client, independently of the Telegram channel platform. With the supported
`generic` value, key creation, exact expiry-instant comparison, callback update
by PUT and readback succeeded at 02:18:48 UTC. The provider returned UTC expiry
with `+00:00`. The disposable key was revoked and its absence verified. No
channel route or Telegram message was created by this check. CMA's route-key
request and metadata validation now use `generic`; the channel stays Telegram.

### NyxID client compatibility rollout and capacity refusal — 2026-09-23

PR #934 merged into `develop` at
`4c06aad4e9ec1870e9d217db0d8c91f4a5f97bf1`. All 14 jobs in CI `35810259814`
passed, including 2,967 regular backend tests, 669 real Mongo contracts, the
Linux writer boundary and all image, frontend and SDK gates. The eight gateway
tests include the dashboard journey through CMA and the compiled gateway,
lifecycle/restart recovery, equivalent offset expiries and cleanup after altered
key metadata. CMA now uses NyxID's existing PUT update methods and `generic`
API-key client platform. It accepts equivalent expiry instants while refusing
changed fractional values; canonical timestamps in CMA remain unchanged.

The API artifact's tested revision
`88577a7c066ea93f48d27ac097006dd7a82da682` and merged revision have identical
tree `cf2e9ed93ab1e181239c23e14d9535162e28d345`. Its archive checksum, image
configuration, architecture and source labels were verified before transfer,
and the immutable registry digest and configuration were read back afterward.
The production API now uses
`cma-api@sha256:8f80d527535370b2150716608629fa97737c2d4236203b9926872256fa072e4f`
under the existing production registry prefix.

At 03:14:58 UTC, the API-only rollout passed its settled health checks. CMA API,
CMAEG and web each had one ready replica. All nine existing runtime Pods retained
their exact UID, full spec, container identity, image, restart count and readiness;
configuration, runtime default and the gateway/web deployment specs were unchanged.
Seven selected Agents passed lineage, profile and durable history-prefix
comparisons. The first post-rollout public reads returned HTTP 503 before the
later checks succeeded. These observations do not establish continuous
availability, and retain the earlier native-conversation, dirty-file, duplicate
input and browser-reconnect coverage limits.

CI's fake-provider p99 measurements were warm launch 34.270 ms, cold launch
64.211 ms, cold restore 57.593 ms and tool delivery 17.612 ms. They are not
Telegram measurements.

The authorized dashboard migration created
`bot_73405efb35c54f349f2bac6918751a10` at 03:16:05 UTC and reached Active by
03:16:09 UTC. Its Agent, Sandbox and Workspace profile pins remained versions
2, 5 and 3 of the original selected profiles. The gateway channel and level-2
provider binding were created successfully, with all requested profile fields
honoured and none refused. This verifies the production Bot creation path; it
does not establish provider readiness or a Telegram round trip.

Provider runtime creation then hit namespace `sandbox-quota`: the new profile
requests 4 CPUs and 8 GiB, while existing requests used 36,875 millicores and
74,816 MiB against limits of 40 CPUs and 80 GiB. Kubernetes rejected creation
of the exact new runtime Pod. The new Bot was disabled and its newly created
route deactivated. At 03:24:03 UTC the original route was restored with its
original key, followed by bridge re-enablement at 03:24:13 UTC, revision 9.
The original Sandbox, Workspace, Worktree, Agent and recorded deliveries remain
intact. The new Bot and its provisioning binding are retained for a capacity
decision; no existing user runtime was stopped or resized.

A proposed change to requests of 44 CPUs and 88 GiB passed Kubernetes server
dry-run but has not been applied. It leaves the quota's CPU/memory limits and Pod
count unchanged and requires the maintainer's capacity decision. No NyxID
catalogue, service configuration, role, code or deployment was changed. No
Telegram test message was sent. The #918 acceptance evidence, #912 independent
documentation exercise and #920 bridge retirement remain incomplete.

### Combined Bot Father and gateway documentation rollout — 2026-09-24

The unified gateway documentation and reviewed owner handoff are deployed and
verified. Father's organization-owned turn passes admission. Its last model
execution now completes and returns a valid owner handoff link after the
authorized organization credit grant on September 25. Telegram cutover is
verified; a genuine user message and its delivered reply remain pending.

PRs #944, #945 and #946 merged into `develop`. The final merged commit is
`c4b026ca59aceee13ea0611c97747d69bd9cbf90`. All 14 jobs in Actions run
`35989386802`, attempt 1, passed for head
`6a263062087f5f893319a3bc065930b7ebb01edb`. The native image artifacts retain
their actual tested revision `1eafc11b0f0fa4a225b0316daebee78e94e4e11e`;
its tree `18c1271c9ddcc1c0ea77eb2282c0157539cdd6c8` equals the merged tree.
Archive hashes, image IDs, source labels and `linux/amd64` platform were checked
before promotion. Registry readback verified the immutable digests and identical
image configurations. CI labels retain their original `development`/`dev` values.

The rollout passed its guarded final verification at 11:27:01 UTC in namespace
`chrono-bot`, cluster
`gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke`.
The registry prefix is
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry`:

| Component | Repository | Verified deployed digest |
| --- | --- | --- |
| CMA API | `cma-api` | `sha256:cd4f473317725d00c0497d674b5d84019fce9ee7a5e2d6333ca5e773e8952749` |
| CMAEG | `cma-cmaeg` | `sha256:5ca9e3608fc87560e05c70dc91d86313fcc3fbf43bb2d547d60fb6808552fe0e` |
| CMA web | `cma-web` | `sha256:09478a1b7745991bc430589eeda55e7bfe1524b3b3fc2497c660750ad10e22f0` |

API, gateway and web each settled at one ready replica. The final observer checked
the installed container image IDs against those registry digests. Live ConfigMaps
and deployment settings other than the three images were unchanged. All six
existing runtime Pods retained their exact UID, full spec, container image and
identity, restart count and readiness. The protected Sandbox
`sbx_9533afb9cb4a46cfb268ced722c65a22` retained Pod UID
`19e9937e-a62a-439b-b2dd-50b3ba71a0ef`. No lifecycle command or runtime replacement
was part of this rollout. These before/after observations establish Pod
continuity; they do not establish uninterrupted request availability or a live
conversation restart drill. The retained runtime default remains
`sha256:532a64cf193957991248d98d82b7a61f6d333a2b87f5e1db0a19ae55f598ed49`.

Public API readiness, gateway readiness, web root and gateway documentation all
returned HTTP 200. A separate gateway check at 11:29:15 UTC confirmed
`ready: true` and `cma_reachable: true`. Rollout evidence is
`/tmp/cma-bot-father-rollout-receipt.json`; image verification receipts are under
`/tmp/cma-bot-father-ci-35989386802/`.

The backend job passed 2,649 regular tests, with 366 skipped in that run,
then 236 real Mongo contracts across six partitions and the explicitly selected
Linux writer-boundary test. Formatting, all-target/all-feature Clippy, workspace
build and the doc-test gate passed; the latter executed zero doctests. Frontend
validation passed 1,429 tests across 154 files, formatting, lint, typecheck,
production build, CSS and style checks. Production dependency installation
reported zero vulnerabilities; the full dependency audit passed its high-severity
threshold with three moderate development dependency findings. Logs, exact job
metadata and checksums are under `/tmp/cma-bot-father-ci-evidence-35989386802/`.

The CI latency harness used the compiled gateway with a fake CMA and a 50-ms fake
delay. Warm launch p99 was 28.557 ms across 500 samples; cold launch p99 61.581 ms,
cold restore p99 56.403 ms and tool delivery p99 20.813 ms each used 200 samples.
All configured limits passed. These measurements are not production Telegram
latency or Bot Father acceptance.

Production browser checks verified the combined documentation at
`https://bot-gateway.chrono-ai.fun/docs`: 39 section links resolve without missing
or duplicate anchors, all ten SVG diagrams decode, desktop and 390px mobile
layouts have no horizontal overflow, and light/dark screenshots were inspected.
Clicking the top-right download button saved the complete 82,380-character
`agent-event-gateway.md`, including Flow A through Flow H. A fresh browser with
its cache disabled opened the downstream-provider Flow F deep link at 101px,
below the sticky header, while earlier images loaded lazily. The two legacy
architecture/protocol URLs return permanent HTTP 308 redirects to the matching
sections. All three raw Markdown endpoints and `/openapi.json` return HTTP 200;
the latter advertises 13 operations.

Browser and HTTP receipts, screenshot hashes and the downloaded document are
indexed by `/tmp/cma-bot-father-production-docs-receipt.json`. The main browser
report is `/tmp/cmaeg-unified-production-browser.json`; screenshots use
`/tmp/cmaeg-unified-production-*.png`, with the uncached link screenshot at
`/tmp/cmaeg-fresh-deeplink.png`. The first dark screenshot preceded lazy-image
decoding; the inspected replacement waits for the image to decode.

The production Father handoff journey passed at 11:32 UTC against the deployed
web and API images. An anonymous draft link required sign-in and made no profile
mutation. After the existing human owner's real sign-in cookie was verified,
the draft survived the clean return and still required explicit review. The
owner confirmed one private Agent Profile, which was created and published as
`agp_b772aa7952c547389e2af4bc37207b3c`, revision 1. The existing Bot wizard
received the chosen name and profile. Its owned Telegram selection, access
policy and final review were reached; the final Create Bot action was not
invoked, so this check created no Bot or route. Cleanup retired exactly that
private profile and was verified at 11:32:50 UTC.

The handoff receipt is `/tmp/cma-bot-father-live-handoff-receipt.json`. Two
earlier observer attempts timed out before dispatching any profile creation:
the receipt records their absent create authority and zero mutation counters.
The browser observer's exact textarea-label selector was corrected before the
successful attempt. This evidence establishes the deployed handoff and cleanup;
it does not establish a real Telegram conversation or completed personal Bot
creation.

At 11:49:16 UTC, a separate read through NyxID's `cmaeg` service proxy returned
HTTP 200 for the organization-owned gateway channel. The observed organization,
gateway record and provider binding all matched the retained Father resources.
The check made no remote mutation. Its receipt is
`/tmp/cma-bot-father-nyx-proxy-receipt.json`; it establishes this authenticated
gateway read, not organization turn execution or ordinary CMA API-key enablement.

This entry records the deployed implementation, public documentation and owner
handoff checks. The subsequent organization-owned Father custom-event check
exposed an API authorization refusal for the organization turn. Personal-only
checks in admission and command authorization discarded the authoritative
binding's organization ownership. PR #947 preserves the complete owner kind
and subject through both paths while retaining exact actor, conversation,
generation, credential and resource-lineage checks.

The correction merged at `378292984840d424906ca5a99ad31e839d6ffcec`, with
tree `23a53ec8249f240cec8b07b1e12a6fdad0a2b9d2`. All 14 jobs in Actions run
`35994566783`, attempt 1, passed for reviewed head
`eb6898cb11533e8e40f61cf0846bb8e97fbee900`. Backend validation passed 2,652
regular tests, 236 real Mongo contracts across six partitions and the explicit
Linux writer-boundary test, plus formatting, Clippy, build and doc-test gates.
The regular pass skipped 366 tests; the doc-test gate executed zero doctests.
Local focused validation also passed 71 tests: 17 CMA, 19 public API and 35 core,
with Clippy and formatting. Independent read-only review by Codex agent
`agent_auth_ship` found no blocking privilege or compatibility gap in ownership,
deferred authorization, command execution or retained personal bindings.
Logs, job metadata and checksums are under
`/tmp/cma-bot-father-ci-evidence-35994566783/`.

The correction run's fake-provider latency p99 was 15.431 ms for warm launch
across 500 samples, and 60.719 ms for cold launch, 55.434 ms for cold restore
and 9.771 ms for tool delivery across 200 samples each. All limits passed;
these remain CI measurements.

The API-only correction passed guarded production verification at 12:20:57 UTC.
Its final deployed image is
`cma-api@sha256:55ded45dd035d7745bffaeea59400320d316bdc6d6f561fbf91dddca7e5e57d3`
under the same registry prefix. Its original tested revision is
`a306f4504c20e9dc0929cc9ff3bd72586ddd2f85`, with the same tree as the #947
merge. Archive, source label, platform, immutable registry digest and image
configuration checks passed. The earlier API digest in the combined rollout
table is superseded by this correction; gateway and web retain that table's
verified images.

All seven current runtime Pods, including the original protected Sandbox and
the new Father Sandbox, retained exact UIDs, full specs, container identities,
restart counts and readiness. Father Pod UID
`6a0d5b2c-6049-47c7-a2fb-59ad2dd1e7bb` was preserved. ConfigMaps, gateway and web
were unchanged; all three public health checks returned HTTP 200. The first
health observer request encountered Cloudflare error 1010 with Python's default
user agent. The same check passed with its explicit
`cma-bot-father-rollout/1` user agent; this was an observer compatibility issue.
Evidence is `/tmp/cma-bot-father-api-fix-rollout-receipt.json` and
`/tmp/cma-bot-father-api-fix-ci-35994566783/`.

At 12:26:28 UTC, the retained custom event successfully passed provider admission
after the correction, in 0.368962242 seconds. This confirms that the prior
organization authorization refusal is resolved. The Father resources belong to
ChronoAI organization `52aa284c-b5b2-49bd-9d12-79f24772dbd7`: gateway record
`77fe02e5-2905-4f00-86ce-5d50a63bfe47`, binding
`bnd_77fe02e529054f0086ce5d50a63bfe47`, Sandbox
`sbx_5dfad475e99d4e0cbe2c387fc0e2d50e`, and Agent
`agt_a410e5bc0d1f4701a05eacb36eb0324d`.

The admitted event `39cab064-3ca4-4763-8b4d-c107306cbd66` produced response
`resp_9e96a0d82d964598af1b84f7a2930b41`, which finished as `failed` during model
execution. Read-only inspection of the Father runtime's `logs_2.sqlite` showed
five sampling retries receiving upstream HTTP 402, `insufficient_credits`,
error code `11300`, with message `Insufficient billing credits`. This is a
distinct billing refusal after successful CMA admission, not the earlier
organization authorization defect. The receipt is
`/tmp/cma-bot-father-model-failure-receipt.json`.

Read-only billing checks at 12:30:37 UTC found zero active credit grants for the
organization and unused daily allowances of 1,000,000 input tokens and 10,000
output tokens. Allowance consumption and reservations were both zero. The exact
runtime request contained 56,407 bytes and no `max_output_tokens`; its SHA-256 is
`b5a0d3a44a78cbaba7320ceb1805f21dc50437e6ea6288ee8c1c281a700b89c6`.
NyxID's current request-bytes/4 estimator rounds this to 14,102 tokens for both
input and output reservation. The input estimate fits the daily allowance, but
the output estimate exceeds its allowance by 4,102 tokens. At the observed
0.00002 credits per output token, that excess is 0.08204 credits, which the
wallet reservation rounds up to one whole credit. This calculation describes
the reservation estimate, not actual model output or a completed debit.

The live organization wallet balance was not established. The organization API
key cannot access the billing endpoint, and the human wallet endpoint has no
owner selector and may create a wallet, so that endpoint was not called.
The evidence therefore does not establish an exhausted wallet. The observed
facts remain the HTTP 402 refusal, unused allowances, zero active grants and
the reservation calculation. Details are in
`/tmp/cma-bot-father-billing-evidence.json`.

After the maintainer's explicit billing authorization on September 25, one
official credit grant was issued at 05:46:21 UTC. Grant
`b3efd9e1-96d6-440a-bcd9-88d2999ecc90`, batch
`cd39fabb-392f-4822-a285-4ed0b051f819`, is active for exactly the ChronoAI
organization and `chrono-llm-public`. Readback confirmed billing enabled,
10,000,000 remaining microcredits, zero reserved microcredits and one matching
grant in the complete organization inventory. The grant is a one-time allocation
of 10 internal credits, not a recurring grant or payment-method charge. Exactly
one financial mutation was made; the organization wallet balance remains
unobserved. Evidence is `/tmp/cma-bot-father-credit-grant-receipt.json`.

The failed event remains durable in `test_history`. Before creating a replacement,
read-only provider verification confirmed its terminal failure, generation 1,
exact event/conversation/binding/Agent/Sandbox lineage and retained event context.
The first post-grant attempt waited for Google Cloud reauthentication and made
no event mutation. After access was restored, replacement custom event
`f5485159-3fb0-487c-9294-b354ba6ad2c1` completed at 05:50:01 UTC with HTTP 200,
`cmaeg.message` output and terminal `cmaeg.completed`. The 1,367-byte answer
contained a valid owner handoff draft link and returned no credential or event
reference. Its SHA-256 is
`6dfccce29263e69cd6eba14ecbdf20c69ccd42ea763540bb9ba0f71dac322472`.
This is successful production custom-event/model/handoff acceptance after the
grant, recorded in `/tmp/cma-bot-father-provision-receipt.json`; it is not a
Telegram message test.

The September 25 Telegram preflight found the Father binding ready at generation
1 with always-keep-alive enabled. The new runtime baseline records the original
protected Pod UID `19e9937e-a62a-439b-b2dd-50b3ba71a0ef`, still ready with zero
restarts. Father now uses ready Pod UID
`26d03403-74ef-4272-8171-a41b339bc48a`, created on September 24 at 15:35:06 UTC,
after the earlier API rollout and before this cutover attempt. The new baseline
does not extend the earlier rollout's Pod-continuity observation across that
replacement. Both current Pods use the retained runtime image and have zero
restarts. Evidence is
`/tmp/cma-bot-father-telegram-runtime-baseline-20260925.json`.

The authorized Telegram cutover completed and passed verification at 05:51:48 UTC.
The existing Telegram bot `548fdeb5-3c1c-4f49-a982-72e19f995507` was transferred
from its previous personal owner to the ChronoAI organization. The preview-fenced
transfer retired both prior routes. New route
`b9973c4e-14a0-4ebf-90b9-27c80be60f81` is the single active route, with the
organization owner, correct retained route key and Father gateway callback.
The gateway's route matches. Evidence is
`/tmp/cma-bot-father-telegram-receipt.json`.

The original CMA Bot `bot_73405efb35c54f349f2bac6918751a10` is disabled with no
pending operation. Its original provider binding, protected Sandbox and
credential-renewal support remain retained; the route migration did not pause or
replace that runtime. Both the protected user runtime and current Father runtime
preserved their exact Pod UIDs, container identities and restart counts against
the September 25 baseline. The read-only observer passed at 05:51:50 UTC and
had not yet seen a new inbound message. The maintainer was asked to send the
agreed Paper assistant test to the existing Telegram bot.

This verifies the deployed custom-event/model/handoff path and Telegram route
configuration. Genuine Telegram ingress, delivered reply and final Bot Father
acceptance remain pending the human test message. These observations do not
establish gateway epic completion or close its remaining acceptance work.

### Organization navigation production verification — 2026-09-25

PR #949 merged at `99ac49185bb7a6ba2b5fc4dd505785491a3ba0ce`. All 14 jobs in
Actions run `36101489523`, attempt 1, passed for reviewed head
`840bfbbdff319d5da14aef6923a92feef8da02e8`. The actual web artifact retains
tested revision `5c40bff701ef03b19f2b04f30a1e891b2d962843`; its tree
`181b3b295dfaa7faa7d818505cef40f4bb2879c3` equals the merged source tree.
Archive checksums, native image identity, source labels, `linux/amd64` platform,
registry digest and identical registry image configuration were verified.

The web-only rollout passed guarded verification at 06:36:50 UTC. The final
deployed web image is
`cma-web@sha256:b15c241f81e9f706762252865183cf20c65e2c8f9375266cab150fb1c835abd6`
under the existing registry prefix. It supersedes the earlier combined rollout's
web pin. The API remains on `55ded45d` and gateway on `5ca9e360`, whose complete
digests are recorded above. API, gateway and web each have one ready replica;
public API readiness, gateway readiness and the web root all returned HTTP 200.

A fresh baseline taken at 06:35:27 UTC fenced the exact production namespace,
deployment `chrono-bot-prod`, database `chrono-bot`, bucket
`chrono-platform-bot`, current runtime default, ConfigMaps and installed
API/gateway Pods. Only the web image changed. All seven current runtime Pods
retained their identities and specs, including protected user UID
`19e9937e-a62a-439b-b2dd-50b3ba71a0ef` and Father UID
`26d03403-74ef-4272-8171-a41b339bc48a`. The installed web Pod UID is
`381c050a-4dc0-4b0b-a88b-f0e1b6aca2b6`; its actual container image ID matches
the final web digest, it is ready and has zero restarts. Rollout evidence is
`/tmp/cma-org-session-web-rollout-receipt.json`.

Real browser verification at `https://bot.chrono-ai.fun` passed at 06:37:01 UTC.
The ChronoAI organization tree exposed Sandbox, Workspace and Agent levels.
Navigation opened the Father Sandbox, Workspace
`wks_fcfc4b10078149a182f8041935e8599d` and Agent
`agt_a410e5bc0d1f4701a05eacb36eb0324d`. The Agent conversation and administrator
composer were visible, reloading the deep link retained organization scope,
the details panel worked, and mobile layout checks passed. The observer recorded
no browser errors and made zero resource mutations. This verifies navigation and
composer availability; it does not claim a submitted conversation or a genuine
Telegram message. Browser evidence is `/tmp/cma-org-session-live-receipt.json`.
