# NyxID dependency contracts and Friday evidence

Part of #1564. This is the Friday source-contract slice, audited on 2026-11-11
against develop `eda350044` and the tests in this PR. The reviewed PR head pins
the resulting source and fixtures. `api/Cargo.lock` pins reqwest **0.12.28**,
Tokio **1.53.3**, axum **0.8.9** and serde_json **1.0.154**; checks use `--locked`.
These are CMA client versions, not a verified NyxID deployment version.

No live identity, credential, provider response or production traffic was used.
The matrix describes CMA's requirements of the configured NyxID origin. It does
not establish upstream acceptance, availability commitments or seven-day evidence.
Release acceptance must record the actual CMA source/image digests and the NyxID
deployment/schema version together. #1564 remains open for that acceptance.

## Endpoint matrix

Source paths below are relative to `api/crates/`. Line numbers identify the
implementation at the audited source; the linked files and test names are the
durable references. Discovery-provided paths are not assumed to be hard-coded.
No endpoint in this table authorizes use of a master key or credential export.

| Dependency / endpoint | CMA request and required response | Bounds and failure contract | Source and local evidence |
| --- | --- | --- | --- |
| Identity discovery: GET `/.well-known/openid-configuration` | No bearer; obtain authorization, token, userinfo and optional introspection endpoints. Cache successful metadata. | Verification includes discovery inside the 2.5 s sequence; complete evidence ≤64 KiB. Direct login discovery uses the 15 s client. Failure is unavailable, never identity evidence. | `cma/src/auth/provider_metadata.rs:23`, `identity_http.rs:12`; new `discovery_identity_and_permission_contract_bind_one_credential` |
| OAuth browser authorization and POST discovered token endpoint (normally `/oauth/authorize`, `/oauth/token`) | Authorization code with state/PKCE and configured redirect; confidential client code exchange, refresh grant, bounded token lifetime. A redirect alone does not start Agent work. | HTTP calls use 15 s; refresh has durable custody/lease checks. This is not a 2.5 s end-to-end login/refresh guarantee. Rejected refresh signs out; outage never invents a refreshed token. | `cma/src/auth/flow.rs:168`, `refresh.rs:115`, `refresh_lease.rs`; existing `flow_tests`, `flow_registration_tests`, `refresh_tests` |
| GET discovered userinfo endpoint + POST discovered introspection endpoint | Both inspect the same immutable bearer; form includes `token`, `token_type_hint=access_token`, confidential client fields. Require matching subject and complete active permissions. If introspection is absent, permissions must be present in userinfo. | Concurrent calls per attempt; ≤3 attempts, 750 ms/attempt, 2.5 s whole sequence, 64 concurrent sequences with no capacity queue. 502/503 or classified transport failures can retry; rejection wins over concurrent outage. Typed `StoreError::Unavailable` and public 503. | `cma/src/auth/identity.rs:105`, `permissions.rs:76`, `identity_retry.rs:16`; new discovery/503/deadline tests, existing cookie/bearer authorization suites |
| POST introspection, ordinary/scoped API key | `token_type_hint=api_key`, configured service ID and confidential client. Require active key, exact audience, `token_type=api_key`, proxy scope, subject/permissions and valid lifetime; no unrelated resource authority. | Same API verification budget; no successful authorization cached on failure. Revoked/unknown keys fail closed. | `cma/src/auth/agent_keys.rs`; existing `agent_keys::tests::http::provider_failure_is_unavailable_and_unconfigured_keys_do_not_reach_nyxid` and `security` tests |
| POST introspection, request-bound proof | `token_type_hint=resource_request`, service ID and exact method/path/query/body digest/content type. Read-only descriptor; no independent commands or ambient custody fallback. | Discovery and introspection each inherit 15 s; ≤64 KiB evidence. No automatic retry in this path. Typed `resource_request_unavailable`/503 for provider failure; rejected or expired proof remains denied. | `cma/src/auth/resource_request/verification.rs:19`, `resource_request.rs:37`; existing `invalid_evidence_and_outages_fail_closed_with_distinct_status`, descriptor/paired/public-route tests |
| GET configured JWKS URL (manifest uses `/.well-known/jwks.json`) | Verify signed proxy identity against configured issuer/audience/service, key ID, signature and time claims; never trust decoded claims alone. | Connect 1 s / HTTP 3 s; ≤32 KiB JWKS. Coalesced refresh with cooldown, bounded cache; unknown key or expired cache fails closed. Lock wait is separate from the HTTP timeout. | `cma/src/auth/nyxid_proxy/jwt.rs:86`; existing `jwt_tests` |
| CMAEG POST `/oauth/introspect` | OAuth access token with exact issuer, resource, expiry and service permission; ordinary service key additionally binds the service ID/audience and key type. | Shared client connect 2 s / total HTTP 4 s, ≤64 KiB, redirects disabled, no application retry. `AuthError::Unavailable` for 503/transport failure; unauthenticated and forbidden remain distinct. | `cma-service-auth/src/nyx.rs:111`, `service_key.rs:65`; new service success/permission/503/4 s tests exercise the public production constructor; existing service-key tests |
| Service browser GET `/oauth/authorize`, POST `/oauth/token` | Each service's own client/resource/redirect, state/PKCE and exclusive browser custody. | Uses the service client above; browser flow/custody adds work outside an individual HTTP deadline. | `cma-service-auth/src/browser.rs:262`; existing browser/session tests |
| GET `/api/v1/public/config` | Public signup options, no credential; unexpected shape cannot enable an unadvertised signup path. | Bounded provider reader and HTTP client; unavailable public options remain a failure/default state. | `cma/src/auth/signup_options.rs:106`; existing malformed, failed and delayed provider tests |
| GET `/api/v1/services` and `/api/v1/user-services` | Catalogue choices versus actual active owner grants are distinct. Resolve requested slugs/IDs to exact active grants; catalogue presence alone grants nothing. | Key client 15 s/call, catalogue ≤1 MiB/1,000 rows; no application retry. 503 is `KeyIssuerError::Refused(503)`. Sandbox custody maps 429/5xx to `CredentialError::Unavailable`. | `cma/src/auth/keys.rs:287`, `keys/catalog.rs`; new issuance stage-zero test; existing key/catalogue and connection tests |
| POST `/api/v1/api-keys/scope-plan` | Selected exact service IDs (plus organization owner when applicable). Require `authority=nyxid`, contract `1`, policy `api-key-scope-v1`, actor/owner, exact services/nodes, complete route basis, freshness and normalized digest. | 15 s/call; no application retry. Stale (>300 s), excessive future time (>60 s), changed owner/grant/digest or incomplete plan fails closed. Mutation must revalidate `scope_plan_digest`. | `cma/src/auth/keys/scope_plan.rs:69,198`; new stage-one wire/503/stall tests; existing `incomplete_stale_cross_owner_or_extra_authority_is_refused` |
| POST `/api/v1/api-keys` | Deployment-owned name, `proxy read`, finite expiry, exact service/node grants, wildcards false and scope-plan digest. Validate returned grant, key ID/value and positive state version. | 15 s/call; personal mint has three sequential HTTP phases (up to about 45 s before surrounding custody work). No automatic mutation replay. Refused503/transportUnavailable cannot become success. Uncertain remote creation still needs reconciliation. | `cma/src/auth/keys.rs:226`, `keys/grant.rs:43`; new exact grant and every-stage failure tests; existing scope-plan, key and credential custody tests |
| GET `/api/v1/api-keys`; DELETE `/api/v1/api-keys/{id}` | Inventory limited to recognizable deployment-owned names; revoke exact validated provider key ID with owner authority. | List ≤1 MiB/1,000 rows; 15 s/call. Revoke 503/timeout is not confirmation; this client also returns typed refusal for 404. No automatic retry at this client boundary; durable cleanup owns later attempts. | `cma/src/auth/keys.rs:266,310`; new revoke success/503/timeout tests; existing `keys_tests` and custody/revocation-debt tests |
| GET `/api/v1/orgs` and `/api/v1/orgs/{id}` | Fresh membership for the actual selected caller credential; canonical organization ID and role, never an ambient owner's credential or catalogue cache. | Whole membership lookup 15 s, response ≤1 MiB. Unavailable, forbidden and invalid evidence remain distinct. Organization bot issuance shares the key endpoints but has its own validation/error mapping. | `cma/src/auth/organizations/mod.rs:39`, `transport.rs`; `keys/bots.rs`; existing organization and bot-key tests |
| Connection catalogue/repair: GET `/api/v1/user-services`, `/api/v1/services`, `/api/v1/keys/{id}`, `/api/v1/assistant/readiness` | Active routing row plus present usable credential; node-backed service requires online node. GitHub material readiness requires revision `nyxid-assistant-readiness.v1`. Do not read provider credential material. | 15 s/call, ≤1 MiB responses and 1,000 catalogue rows; no application retry. 5xx/408/425/429 => `ServiceConnectionError::Unavailable`; invalid or missing state cannot satisfy a prerequisite. Several required services are not one 15 s operation. | `cma/src/trigger/connections_nyxid.rs:110,132`, `credentials.rs:28`, `composition.rs:14`; existing `credential_metadata_failures_never_look_like_satisfied_connections` and catalogue tests |
| POST `/api/v1/connect-links`; GET `/api/v1/connect-links/{id}` | Owner bearer, service slug, 900 s TTL and approved callback; require canonical provider browser origin, matching link/service and completed connection evidence. API-key/OAuth/device-code entry is delegated through this browser contract. | 15 s/call, bounded responses; no immediate retry. Expired/cancelled link is not acceptance. Browser completion is followed by credential readiness verification. | `cma/src/trigger/connections_nyxid.rs:293,319`; existing connect-link, callback, malformed and expiry tests |
| Model catalogue GET `/api/v1/proxy/s/{chrono-llm-public or chrono-llm}/models` | Installed Sandbox scoped credential, JSON model rows; exact service path. Catalogue presence alone is not model availability/acceptance. | 15 s including body; ≤1 MiB/1,000 rows, redirects disabled, no application retry. `AgentModelCatalogError::Unavailable` plus fixed ModelGateway reason for 503/timeout. | `cma/src/agent_models.rs:27,44`; new wire/success/503/15 s tests plus existing catalogue/diagnostic tests |
| Runtime model proxy: configured `/api/v1/proxy/s/{service}` prefix + client route, e.g. POST `/v1/responses` | Inject current installed credential; discard caller credential/routing authority. Preserve permitted response status, headers and streamed body; guard against credential echo. | 600 s total upstream HTTP deadline, redirects disabled, no application retry. 503 and its supplied typed error/Retry-After are preserved. Pre-header timeout returns fixed HTTP 502; post-header timeout is a body error. These are not a generated JSON error code or recoverable terminal SSE event. | `layer4-runtime/src/upstream.rs:205,227`; new proxy success/503/header/stream tests; existing `upstream_tests` and `upstream_guard` tests |

The identity fallback policy from #1411 remains deliberately narrow: only
GET `/auth/session` may use an existing process-local successful observation for
at most 60 s since the last online validation, capped by token expiry. The cache
holds at most 1,024 records of at most 16 KiB, has no sliding TTL, and cannot be
populated by failures. Writes, creation, other API reads and worker authorization
fail closed. Capacity exhaustion, invalid/incomplete evidence and explicit
revocation never enable fallback. See `auth/identity_fallback.rs:13,43` and
`cookie_authorization_tests/fallback_tests.rs` for recovery and invalidation tests.

For bearer identity, a transient outage maps to
`StoreError::Unavailable("identity_provider_unavailable")`; other unusable evidence
still maps to `StoreError::Unavailable`, with a different internal detail.
Callers must use the typed/public error contract, not parse internal strings.
The retry policy does not retry all 5xx: 500/504/429 and malformed/incomplete
evidence are unavailable without transient retry. This distinction is deliberate.

## Deterministic checks added by this slice

| Test file | Guarantees exercised |
| --- | --- |
| [Identity and service contracts](../api/crates/cma/src/auth/nyxid_contract_tests.rs) | Exact discovery/userinfo/introspection wire contracts; success and permission refusal; identity 503 stops after three paired attempts and recovers on a new request; total cancellation deadline; production service client 4 s deadline and one-request 503 behavior. |
| [Scoped issuance contracts](../api/crates/cma/src/auth/keys/nyxid_contract_tests.rs) | Exact catalogue → scope plan → mint → revoke sequence; 503 at each issuance stage prevents later calls; held headers/body return typed unavailable at the real 15 s client deadline; uncertain revoke never acknowledged as success. |
| [Model catalogue contracts](../api/crates/cma/src/agent_models/nyxid_contract_tests.rs) | Exact scoped endpoint/auth, parsed catalogue, fixed 503 diagnostic and production 15 s timeout without retry. |
| [Runtime proxy contracts](../api/crates/layer4-runtime/src/upstream_nyxid_contract_tests.rs) | Actual proxy constructor, installed authority and prefixed route; success and supplied typed 503 preserved; production 600 s header/body cancellation with no replay or invented successful stream completion. |

All added fixtures use synthetic values and ephemeral loopback listeners only.
Request barriers establish provider contact before advancing Tokio's virtual
clock. The runtime stream test waits for received response headers before testing
body failure. The catalogue fixture accepts either header/body timeout diagnostic
when headers are still in flight, while enforcing the same typed failure/deadline.
There are no sleep-based phase assumptions or external service credentials.
Watchdogs fail hangs; request counts check retries and cancellation. Timer bounds
allow only Tokio's one-millisecond deadline rounding. Production timeouts are not
shortened or overridden in these tests.

## Explicit real-window and remaining coverage list

1. **Versioned owner contract.** Obtain NyxID deployment/schema versions, endpoint
   ownership, availability/rate-limit/quota commitments, support/escalation and
   maintenance behavior. The source matrix cannot make upstream commitments.
2. **Real credential classes and isolation.** OAuth code/refresh, API keys,
   device-code/connect links, personal and organization scoped keys, request-bound
   proofs and signed proxy identities: happy paths, wrong owner/service/audience,
   permissions, revoked/expired credentials, clock skew and JWKS rotation.
   Use approved test principals; retain aggregate results, never credential values.
3. **Outage and recovery under load.** 503, no headers, stalled/truncated body,
   DNS/TLS/connect failures and schema drift at the actual candidate digests.
   Verify ≤3 s identity route p99, ≤64 concurrent sequences/no queue, bounded
   fallback ≤60 s without sliding, writes fail closed and real recovery. Local
   timers/request counts do not prove fleet-wide p99 or load-balancer behavior.
4. **Scope-plan mutation race and uncertain writes.** NyxID must reject changed
   grants after planning, enforce exact nodes/services/owner and digest binding,
   honor expiry/revocation and provide reconciliation after a lost mint/revoke
   response. Local fixtures cannot prove a timed-out remote mutation did not commit.
   Organization bot issuance and owned-key listing do not yet have the full
   new per-stage 503/timeout matrix; their existing schema/security tests remain.
5. **Long model streams and terminal failure.** Test real model selection,
   >600 s streams, client disconnect, upstream throttle, interruption and recovery.
   The current 600 s upstream limit and post-header body error do not satisfy a
   promised >600 s recoverable terminal-event contract. Agree and implement any
   required deadline/terminal policy separately; this PR preserves current behavior.
6. **Whole-operation bounds beyond this slice.** Per-call 15 s issuance/connection
   limits are not a universal fail-fast route SLO. Discovery, retained credential
   renewal, organization lookup, mutex/lease wait and multiple services can add
   latency. Runtime inbound request-body collection is size-limited but lacks its
   own explicit time deadline. JWKS lock wait also is not its HTTP timeout.
   These paths need end-to-end budget tests and fixes before claiming no-hang
   guarantees for every public request. They are explicit gaps, not passed tests.
7. **Adjacent machine clients.** Storage OAuth/client-credentials and Ornn routing
   are covered separately by [Dependency_Contracts.md](Dependency_Contracts.md)
   and #1569. Paid funding endpoints/token refresh belong to #1566. This matrix
   does not establish their upstream acceptance or enable paid/channel features.
8. **Real observation window and release receipt.** Schedule the NyxID contract
   window with the dependency owner, then collect the #1564 seven-day matrix
   (2026-11-23 through 2026-11-29) and the earlier versioned contract evidence.
   Friday 2026-11-13 delivery is this reviewed source/test slice; train deployment
   and production verification require separate receipts. No live acceptance
   window is claimed to be booked by this document.
