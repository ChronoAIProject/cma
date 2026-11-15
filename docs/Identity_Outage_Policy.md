# NyxID interruption policy

CMA obtains live identity-provider evidence for every authenticated request.
Only callers admitted while an identical credential's verification is unfinished
may share its result. Later callers start a new verification. Completed success
is not cached, including for `GET /auth/session` during an outage.

This replaces the earlier session-only 60-second successful-evidence fallback.
An exhausted outage now fails closed even when a previous request succeeded.
Completed-success reuse is deferred to #2002, which requires an authoritative
revocation or permission version checked by every reuse consumer. A short TTL,
a default-off setting or best-effort cross-replica invalidation does not establish
that contract. This release exposes no completed-reuse configuration setting.

## Fresh authority and live sharing

Cookie flight keys bind the exact token digest, owner, subject incarnation,
token generation and expiry. Independent bearer flights bind the exact credential
digest. Provider configuration belongs to the AuthState. Only unfinished flights
coalesce; the map releases a completed flight and a later request cannot discover
or borrow its result. An already-admitted waiter holds the same immutable receipt
for its own authorization, never a newer result found by looking up the token.

Successful receipts carry a process-keyed token fence and their original token
expiry, bounded by both wall and monotonic clocks. Each new successful verification
supersedes older live receipts for the same token. Observed refusal, replacement,
refresh or logout fences pending verification and held receipts. Cookie custody
is checked after provider work, and browser login is rechecked after that I/O,
including logout committed by another replica. A final request-local check rejects
a superseded or expired receipt after those reads. The registry retains only weak
invalidation markers, not successful userinfo, permissions or profile payloads.

Each replica verifies later requests online, so a prior success on replica B
cannot conceal a revocation observed on A from B's next verification. This does
not promise that already-admitted work can discover an undelivered upstream change
without another authoritative observation. Commands, resource authorization and
stream lifetimes keep their separate current-authority checks.

## Retry and refusal

One shared verification sequence permits at most three attempts inside 2,500 ms.
Each attempt has a 750 ms budget for discovery if needed and concurrent userinfo
and introspection. Connection/setup failures, pre-response timeouts and HTTP
502/503 permit bounded retry. A response body that starts but cannot complete or
validate within the attempt is invalid evidence, not a retryable transport outage.

HTTP 401/403 and explicit revoked/inactive/expired/error dispositions refuse
authority. Either concurrent endpoint's explicit refusal wins over throttling
or a transient sibling; outstanding work is cancelled. Wrong subjects, malformed
claims, duplicate fields, partial successful evidence and local overload fail
closed. No provider error can authorize from a previous request's success.

HTTP 429 is classified before checks for incomplete successful evidence. Legal
advisory headers such as `Link` cannot hide its backoff metadata. Numeric or
HTTP-date `Retry-After` is respected; multiple hints and simultaneous endpoint
throttles use the longest remaining delay. Missing/malformed hints use one second.
A valid zero hint still receives bounded backoff: 40 ms then 80 ms, plus 0–30 ms
jitter. Provider delay is never shortened to fit CMA's budget.

Another attempt starts only when its delay plus a complete attempt fit the
remaining budget. Otherwise CMA returns the final typed throttle immediately
with its remaining delay. Caller cancellation leaves no detached retry work.
There are at most 64 admitted provider sequences, bounded 256-entry flight maps
and 1,024 active-token weak fences per process; distinct-token overload does not
accumulate a queue or bypass verification.

## Public error contract

Exhausted provider throttling on ambient API authentication returns HTTP 503,
`application/problem+json`, `code: identity_unavailable`, `retryable: true`,
`Cache-Control: no-store` and the remaining `Retry-After`, rounded up to at least
one second. An upstream provider 429 is not an inbound API 429 or a guard-capacity
refusal.

Exhausted transient verification on `GET /auth/session` uses the same identity
error with `Retry-After: 3`. Other authentication-store failures retain
`store_failed` and `retryable: false`. The shared public fixture and existing SDK
replay pin the additive retryability metadata. Client retry behavior does not
authorize replaying user mutations or submitting drafts automatically.

## Observation and release

`cma_identity_provider_calls_total{endpoint,route}` counts verification dispatch
attempts. `cma_identity_provider_results_total{endpoint,route,outcome}` records
response-header status, transport failure or cancellation; HTTP 200 is a header
observation, not proof that the body and authority validated.
`cma_identity_verifications_total{route,source}` distinguishes led and shared
callers. Do not sum callers or results into upstream dispatches.

These counters cover the shared verification reader: discovery, userinfo,
introspection and Agent-key verification. They do not cover token exchange,
organization APIs or JWKS traffic. Labels use fixed endpoint/route categories;
other routes are grouped as `other`. Tokens, token digests, personal data and raw
resource IDs are not metric labels. A dispatch attempt is not a proven wire delivery.

Compare per-replica and fleet aggregates with
`sum by (endpoint, route) (increase(cma_identity_provider_calls_total[1m]))`
and the same expression on results filtered by `outcome="429"`. Retain source,
image, replica count, observation bounds and request volume. Completed phase logs,
shared callers and inbound final responses remain separate populations. Production
before/after claims require matched authorized windows; local fixtures establish
behavior only. Sequential reads still verify online; overlapping requests can
share a single provider sequence.

This change introduces no durable format, migration, provider mutation or runtime
image. Release and rollback use the normal authorized API image process. Rolling
back to an image with the old session outage fallback changes its stale-authority
policy and therefore requires the corresponding owner/security disposition.
