# Ornn and object-storage dependency contracts

Friday source-contract slice of #1569; this does not close the external-dependency
gate. Audited against develop `304b44fd9b9d75d282399ccd6ea34e035eb632ca`
on 2026-11-11. Bounded-token and provisional maintenance contracts are documented
with their source and fixture evidence below. The reviewed PR head pins the
resulting source and fixtures. `api/Cargo.lock` pins reqwest **0.12.28**
and Tokio **1.53.3**; validation uses `--locked`.

Release acceptance must additionally bind these contracts to the train's actual
source SHA and API/runtime image digests. No production verification, provider
release version, availability/quota commitment or GCS bucket topology is inferred
from local fixtures.

## Client boundaries

| Candidate operation | Pinned request/response contract | Bounds and failure |
| --- | --- | --- |
| Ornn skill resolution | GET `/api/v1/proxy/s/ornn-api/api/v1/skills/{id}?version={version}` through the configured NyxID origin. Uses the admitted Sandbox credential. Envelope `data` supplies matching `guid`, canonical literal `version`, safe `name`, and lowercase 64-hex `skillHash`; non-null `error` is refused. Mutable tags resolve once into persisted literal version/digest material. | At most 32 distinct references, four lookups in flight; 15 seconds per request including body, 30 seconds for the whole resolution including credential lookup. Maximum 1 MiB metadata. No client retry. 403/404 become `NotFound`; 401/408/429/5xx become `Unavailable`; malformed/pin-changing material becomes `Invalid`. No partial collection on error. |
| Ornn authoring catalogue | GET `.../ornn-api/api/v1/skill-search` with keyword query, public/mixed scope, limit and opaque cursor; then GET `.../skills/{id}/versions`. Preserve pagination, literal releases and owner scope. | Four version lookups in flight; 15-second requests, 30-second total including owner-token lookup; 1 MiB response cap and bounded rows/releases. Outages return `AuthoringSourceError::Unavailable`; malformed envelopes return `InvalidResponse`. No client retry. |
| Runtime skill archive | Persisted literal version forms `.../skills/{id}/versions/{version}/download`; runtime uses its authorized NyxID loopback proxy and verifies the persisted SHA-256 before extraction. | Existing supervisor HTTP client has a 120-second request limit. Archive 64 MiB, expanded contents 256 MiB, 4,096 ZIP entries; unsafe paths and checksum mismatches are refused. This is a separate download/extraction contract, not covered by the metadata-resolution 30-second guarantee. |
| Object-storage token | POST `/oauth/token`, form grant `client_credentials`, configured machine identity and scopes `proxy object:create object:read object:download`. Accept nonempty `access_token` up to 16 KiB and `expires_in` 120–86,400 seconds. | Token envelope capped at 16 KiB. One shared refresh runs independently of its waiters, with a 3-second connection and 12-second request limit. Failed refreshes have exponential backoff (nominally 2 seconds, doubling to 30 seconds; 75–100% jitter); calls needing a token fail fast inside that window. Below 20% remaining lifetime, a still-usable token is served while one background refresh runs; at 60 seconds remaining or less, callers wait. Success resets backoff and advances the token generation. Every caller waits within its own 30-second operation budget. Transport/status/invalid-token failures are `Unavailable`; an oversized envelope is `Corrupt` for shared waiters, then negatively cached as `Unavailable`. |
| Object upload | POST `/api/v1/proxy/s/chrono-storage-service/api/buckets/{bucket}/objects`, content-addressed key, `mutationGeneration=1`, `contentSha256`, Markdown content type. Receipt must match key, byte count, digest, generation 1 and fence version 1. | 3-second connection, 12-second HTTP, 30-second total operation limits. At most two object attempts, only after the first returns 401, with a fresh/replaced token. No replay on timeout, 429 or 5xx. Wrong receipt is `Corrupt`; unavailable service is `Unavailable`. |
| Object download | GET the same proxy's `/objects/download?key=...`, using the cached/current machine token. No caller-supplied storage URL. | Same connection/request/total deadlines. At most one transport retry for connection failures, request timeouts (including body reads), or 502/503/504, after 100–300 ms jitter and only with at least 3 seconds remaining. At most one 401 refresh/replay across all attempts; only rejection of the current generation invalidates the cache. A transport retry plus a 401 replay permits at most three object requests. Bounded document body; redirects and oversize bodies are rejected. Configuration absence uses `UnavailableDocuments`, not an in-memory success fallback. |
| Object delete | **Provisional, unverified E1 contract; default off.** DELETE the same proxy's `/objects?key=...`. A separately instantiated maintenance token requests provisional scopes `proxy object:read object:list object:delete`; online scopes remain unchanged. Only generated document object keys are accepted. | `CHRONO_DOCUMENT_OBJECT_DELETION_ENABLED=false` refuses both maintenance operations before token lookup or HTTP. When explicitly enabled for contract testing, only 200 and 204 map to `Deleted`, 404 to `Absent`, and all other statuses (including 202 Accepted) to `Unavailable`. A 429 exposes a request-local optional retry delay (seconds or HTTP date), capped at 3,600 seconds with past dates treated as zero, through `BlobMaintenanceError`; the caller's worker backs off. No inline transport/status retry; at most one 401 refresh/replay. Redirects are disabled. Connection/request/total budgets remain 3s/12s/30s, including token waits. Storage-owner confirmation and NyxID scope grants are required before enabling in production. |
| Object list | **Provisional, unverified E1 contract; default off.** GET the same proxy's `/objects?prefix=...&cursor=...&limit=256`, expecting `{"data":{"keys":[...],"next":null}}`. Prefixes are limited to `documents/{64-lowercase-hex}/` or that owner's `doc_{32-lowercase-hex}/`. Cursor values are opaque query data, never destinations. | Same gate, separate token, deadlines, redirect refusal and retry rules as delete. Response bodies are capped at 256 KiB, pages at 256 keys and cursors at 4 KiB. Malformed envelopes, non-null errors, excessive pages and keys outside the requested prefix are `Corrupt`. Bad request keys/prefixes/cursor bounds are `Invalid` without HTTP. Listing completeness, including hidden object versions, is unverified; an empty page is not proof of physical erasure. |

The GCS dependency named in #1569 crosses **chrono-storage-service** in this
candidate. CMA's source describes that service as S3-backed and contains no direct
GCS object-client contract. The storage owner must identify the actual backend and
provide its GCS/S3 mapping, version, scopes, quota and recovery evidence. The
Cloud Build/logging client in Image Manager is a different dependency; it does
not establish GCS object-storage acceptance. Retired Sandbox checkpoint schemas
do not authorize new archive capture or substitute for document storage.

## Source and deterministic evidence map

| Contract | Implementation | Tests |
| --- | --- | --- |
| Metadata pins, 32/four/15s/30s, error taxonomy | `api/crates/cma/src/sandbox/agent_skills.rs:12`, `:125`, `:204`; `agent_skills/http.rs:3` and `:37` | `agent_skills_tests/dependency_contract_tests.rs`: 32 exact pinned references with reverse completion, rejected 33rd reference, unavailable status matrix with one request, four held requests cancelled at 15s with no remaining work scheduled, stalled body, credential lookup and successive HTTP batches stopped by one 30s budget. Existing tests retain digest/identity mismatch, scope refusal and no-partial-result coverage. |
| Catalogue request and total budgets | `api/crates/cma/src/profile_authoring/http.rs:8`; `mod.rs:108`; `skills.rs:53` | `profile_authoring/dependency_contract_tests.rs`: unavailable status matrix without retry; held search and version body; owner-token wait. Existing `profile_authoring/tests.rs` pins search envelopes, pagination, literal versions, row/byte limits and owner credential behavior. |
| Runtime archive pins and bounded extraction | `api/crates/layer4-runtime/src/skills.rs:98`; `supervisor_startup.rs:146` | Existing `skills_tests.rs` and `agent_skill_install_tests.rs` cover archive/pin safety. This PR does not add or claim a runtime download-outage acceptance run. |
| Storage operation deadlines, shared refresh and bounded retries | `api/crates/cma/src/document_storage.rs`; `document_storage/token.rs` | `document_storage_token_tests.rs`: 100 concurrent callers, first-waiter cancellation, negative-cache backoff and reset, early refresh, hard expiry margin, stale-generation rejection and bounded waits without late object calls. `document_storage_transport_tests.rs`: connection/status recovery, retry exhaustion, insufficient remaining budget, PUT refusal to replay transport failures, and a single 401 replay across transport attempts. `document_storage_failure_tests.rs` retains token-form, invalid-envelope, 401, held-body/header and shared-budget coverage. |
| Storage authority, receipts and disabled composition | `api/crates/cma/src/document_storage.rs`; `config/documents.rs:12`; `telemetry.rs` | Existing `document_storage_tests.rs`: rejected receipts/redirects/oversize bodies; invalid paths and wrong content never contact storage; successful refresh/cache reuse. `document_storage_receipt_tests.rs` verifies per-field receipt mismatch counters, `Corrupt`, and warnings containing only the field name, without private request context. Configuration tests require the complete canonical deployment config. |
| Provisional maintenance capability, authority and pagination | `api/crates/layer2-core/src/document_artifacts/blob_maintenance.rs`; `api/crates/cma/src/document_storage/maintenance.rs`; `composition/documents.rs` | `document_storage_maintenance_tests.rs`: independent maintenance/online token scopes, zero requests while disabled, deletion completion limited to 200/204, rejected 202 and other success statuses, idempotent 404, rejected foreign prefixes, bounded pages and opaque cursors, malformed/foreign rows, shared 30-second budget and no late calls. `document_storage_maintenance_retry_tests.rs`: request-local Retry-After capped at one hour for huge seconds and far-future dates, zero delay for past dates, one 429 request, refused redirects, bounded chunked/stalled bodies and fixture-only rewrite after deletion. `blob_maintenance_tests.rs`: deterministic deletion, rewrite, isolation and pagination. `config/features/document_maintenance_tests.rs`: independent strict boolean, default off. These are local provisional-contract evidence only. |

The added HTTP fixtures bind only ephemeral loopback ports and use synthetic
authority. Production client constructors retain their configured timeout and
redirect behavior. Request barriers establish that a call reached the fixture;
paused Tokio time then exercises deadlines without wall-clock sleeps, allowing
only Tokio’s documented one-millisecond timer rounding.
Watchdogs fail a hung test, and request counts detect unexpected retries or
work continuing after cancellation. No external services or credentials are used.

## Storage deadline behavior

Both `put` and `get` have one 30-second budget covering token-state lock wait,
shared refresh, object attempts, retry delay and response validation. Token
refresh is single-flight and runs in a spawned task with its own 12-second
HTTP deadline. Cancelling a waiter never cancels that shared refresh, and its
completion cannot issue an object request for an expired or cancelled caller.
Failures are negatively cached for a nominal 2-second window, doubling after
each consecutive failure up to 30 seconds, with 75–100% jitter. Success resets
the failure count. A token below 20% remaining lifetime is returned while one
background refresh starts, provided it still has more than 60 seconds left.
Inside that hard margin callers wait for refresh instead.

An object request can refresh/replay once after 401; a stale rejected generation
cannot evict a newer token. Downloads alone can retry one connection failure,
request timeout or 502/503/504 after 100–300 ms jitter, provided at least 3 seconds
remain. This allowance is shared across the complete download, including body
reads. Uploads never retry transport failures or 5xx. Receipt mismatches return
`Corrupt`, increment `cma_document_storage_receipt_mismatch_total` by field
(`key`, `length`, `generation`, `digest`, `fence`), and log only the field name.
These online token, download and receipt bounds add no configuration key, Secret,
permission or deployment prerequisite. The separately gated maintenance client
has its own pending authorization contract described above.

A timed-out upload may already have reached the service. It remains an uncertain
write and cannot be acknowledged as saved. The existing content-addressed key,
generation fence and durable metadata/idempotency path still govern recovery.
The deadline does not cancel remote side effects or reclaim orphan blobs.

## Explicit remaining acceptance gaps

The E1 object-maintenance contract awaits the storage owner's answers and pinned
service evidence for all of the following:

- Delete endpoint and semantics, including whether 404 proves absence.
- Versioning or soft deletion and the lifetime of non-current versions.
- Whether the same content-addressed key can be rewritten after deletion.
- The maximum landing delay of an abandoned PUT and whether deletion fences it.
- Listing pagination and completeness, including hidden versions and owner scope.
- Exact delete/list scope names and the corresponding NyxID grants (E2).
- Rate limits, 429 behavior and Retry-After representation.
- The real backend's GCS or S3 mapping and pinned implementation version.

Until those answers and grants are verified, keep the maintenance switch off.
No purge, reclaim or account-erasure worker consumes the maintenance port.
Each future caller must record a typed blocked state when
`deletion_supported()` is false.
Synthetic delete/rewrite fixtures do not establish production rewrite semantics,
physical deletion, listing completeness or a zero-residual-storage guarantee.

- Dependency-owner availability, quota/capacity and version commitments for the
  real Ornn and object-storage service, including its actual GCS backend mapping.
- Candidate-digest production evidence for Ornn's 32-reference/four-concurrent
  latency target, scoped authorization, 404/outage recovery and promised real
  journeys; fixtures are not provider acceptance.
- End-to-end runtime archive fetch/install outage, extraction and cancellation
  evidence at the candidate runtime digest, including no partial installation.
- Real storage token expiry/revocation/401 recovery and concurrent refresh
  behavior under load; local attempt limits are not a fleet-wide admission cap.
- Production upload-loss/metadata-commit-loss reconciliation, orphan discovery
  and recovery, verified generation fencing, storage permission and quota faults.
- Full public-route latency including authentication, Mongo and service locks;
  the 30-second guarantee applies to this dependency operation only.
- Deploy and verify this deadline change through the release train. Source-only
  tests cannot close #1569. Third-party channels and their 24-hour activation
  acceptance remain separately tracked in #1571.
