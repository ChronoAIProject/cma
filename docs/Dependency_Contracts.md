# Ornn and object-storage dependency contracts

Friday source-contract slice of #1569; this does not close the external-dependency
gate. Audited against develop `304b44fd9b9d75d282399ccd6ea34e035eb632ca`
on 2026-11-11, plus the storage deadline change in this PR. The reviewed PR head
pins the resulting source and fixtures. `api/Cargo.lock` pins reqwest **0.12.28**
and Tokio **1.53.3**; validation uses `--locked`.

Release acceptance must additionally bind these contracts to the train's actual
source SHA and API/runtime image digests. No production verification, provider
release version, availability/quota commitment or GCS bucket topology is inferred
from local fixtures.

## Client boundaries

`CHRONO_DOCUMENT_STORAGE_RESILIENCE_ENABLED` is strict and defaults to `false`.
The token and download retry behavior in the table below requires explicit
activation. While off, the predecessor serialized token refresh/cache, 60-second
expiry margin, one 401 replay and no GET transport retry remain in effect. Both
policies retain the 30-second total operation budget, bounded bodies, unchanged
scopes and receipt rejection. Disable the switch on every API process to roll
back the policy; there is no persistent data migration. Receipt mismatch
counters and field-only warnings activate with the resilience policy.

| Candidate operation | Pinned request/response contract | Bounds and failure |
| --- | --- | --- |
| Ornn skill resolution | GET `/api/v1/proxy/s/ornn-api/api/v1/skills/{id}?version={version}` through the configured NyxID origin. Uses the admitted Sandbox credential. Envelope `data` supplies matching `guid`, canonical literal `version`, safe `name`, and lowercase 64-hex `skillHash`; non-null `error` is refused. Mutable tags resolve once into persisted literal version/digest material. | At most 32 distinct references, four lookups in flight; 15 seconds per request including body, 30 seconds for the whole resolution including credential lookup. Maximum 1 MiB metadata. No client retry. 403/404 become `NotFound`; 401/408/429/5xx become `Unavailable`; malformed/pin-changing material becomes `Invalid`. No partial collection on error. |
| Ornn authoring catalogue | GET `.../ornn-api/api/v1/skill-search` with keyword query, public/mixed scope, limit and opaque cursor; then GET `.../skills/{id}/versions`. Preserve pagination, literal releases and owner scope. | Four version lookups in flight; 15-second requests, 30-second total including owner-token lookup; 1 MiB response cap and bounded rows/releases. Outages return `AuthoringSourceError::Unavailable`; malformed envelopes return `InvalidResponse`. No client retry. |
| Runtime skill archive | Persisted literal version forms `.../skills/{id}/versions/{version}/download`; runtime uses its authorized NyxID loopback proxy and verifies the persisted SHA-256 before extraction. | Existing supervisor HTTP client has a 120-second request limit. Archive 64 MiB, expanded contents 256 MiB, 4,096 ZIP entries; unsafe paths and checksum mismatches are refused. This is a separate download/extraction contract, not covered by the metadata-resolution 30-second guarantee. |
| Object-storage token | POST `/oauth/token`, form grant `client_credentials`, configured machine identity and scopes `proxy object:create object:read object:download`. Accept nonempty `access_token` up to 16 KiB and `expires_in` 120–86,400 seconds. | Token envelope capped at 16 KiB. One shared refresh runs independently of its waiters, with a 3-second connection and 12-second request limit. Failed refreshes have exponential backoff (nominally 2 seconds, doubling to 30 seconds; 75–100% jitter); calls needing a token fail fast inside that window. Below 20% remaining lifetime, a still-usable token is served while one background refresh runs; at 60 seconds remaining or less, callers wait. Success resets backoff and advances the token generation. Every caller waits within its own 30-second operation budget. Transport/status/invalid-token failures are `Unavailable`; an oversized envelope is `Corrupt` for shared waiters, then negatively cached as `Unavailable`. |
| Object upload | POST `/api/v1/proxy/s/chrono-storage-service/api/buckets/{bucket}/objects`, content-addressed key, `mutationGeneration=1`, `contentSha256`, Markdown content type. Receipt must match key, byte count, digest, generation 1 and fence version 1. | 3-second connection, 12-second HTTP, 30-second total operation limits. At most two object attempts, only after the first returns 401, with a fresh/replaced token. No replay on timeout, 429 or 5xx. Wrong receipt is `Corrupt`; unavailable service is `Unavailable`. |
| Object download | GET the same proxy's `/objects/download?key=...`, using the cached/current machine token. No caller-supplied storage URL. | Same connection/request/total deadlines. At most one transport retry for connection failures, request timeouts (including body reads), or 502/503/504, after 100–300 ms jitter and only with at least 3 seconds remaining. At most one 401 refresh/replay across all attempts; only rejection of the current generation invalidates the cache. A transport retry plus a 401 replay permits at most three object requests. Bounded document body; redirects and oversize bodies are rejected. Configuration absence uses `UnavailableDocuments`, not an in-memory success fallback. |

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

The added HTTP fixtures bind only ephemeral loopback ports and use synthetic
authority. Production client constructors retain their configured timeout and
redirect behavior. Request barriers establish that a call reached the fixture;
paused Tokio time then exercises deadlines without wall-clock sleeps, allowing
only Tokio’s documented one-millisecond timer rounding.
Watchdogs fail a hung test, and request counts detect unexpected retries or
work continuing after cancellation. No external services or credentials are used.

## Storage deadline behavior

Both `put` and `get` have one 30-second budget covering token-state lock wait,
token acquisition, object attempts, retry delay and response validation. With
`CHRONO_DOCUMENT_STORAGE_RESILIENCE_ENABLED=true`, token refresh is single-flight
and runs in a spawned task with its own 12-second HTTP deadline. Cancelling a
waiter never cancels that shared refresh, and its
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
The default-off process switch is the only new deployment key; existing machine
credentials and online scopes are unchanged. The policy requires separate
dependency qualification before activation.

A timed-out upload may already have reached the service. It remains an uncertain
write and cannot be acknowledged as saved. The existing content-addressed key,
generation fence and durable metadata/idempotency path still govern recovery.
The deadline does not cancel remote side effects or reclaim orphan blobs.

## Explicit remaining acceptance gaps

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
