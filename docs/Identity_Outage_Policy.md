# NyxID interruption policy

CMA verifies identity on every authenticated request. Completed evidence never
skips a provider call. This policy addresses a temporary NyxID interruption at
the browser session inspection boundary; it does not provide offline API access.

## Threat model and residual access

A caller may hold a token that NyxID revoked just after a successful check. A
provider/network outage can temporarily prevent CMA from observing that
revocation. Only the exact same token, in a still-valid CMA browser login, may
reuse successful evidence, only on `GET /auth/session`, and only on the process
that observed it. During that window the caller can read the previously verified
subject, profile and capability presentation and keep already rendered drafts
and content on screen. Every API read, write, command and privileged action
continues to require live authority. This does not extend an existing stream's
lifetime or change its separate authorization rules.

The worst-case stale session inspection window is 60 seconds from the last
complete successful verification, shortened by the earliest known token expiry.
There is no fallback for a new token, an empty process store, a rejected token,
missing browser custody, or missing verified/durable expiry. Fallback use does
not advance that clock. NyxID 401/403, inactive tokens, mismatched subjects and
invalid/expired lifetime evidence invalidate the token's record. In particular,
introspection's 401/403 now rejects the identity boundary even when it may mean
CMA client configuration is wrong. No rejection is converted into offline access.

Malformed evidence, 429, other HTTP errors and local overload also fail closed.
They cannot trigger fallback. A 429 is neither retried nor accepted using stale
evidence: CMA cannot safely infer whether it represents a user, client, quota or
abuse limit, and must not bypass it or amplify the throttled request. The existing
`store_failed` projection for this refusal is not automatically retried by the
web; only typed identity outages and transport/gateway failures auto-retry.

## Retry and store parameters

- One verification sequence is shared by overlapping callers using the existing
  cookie/bearer single-flight mechanism. Later requests always verify again.
- Up to three attempts; each attempt has a 750 ms deadline covering discovery
  if needed plus concurrent userinfo/introspection. Total sequence and caller
  wait are capped at 2,500 ms. Delays are 40 ms then 80 ms plus 0–30 ms jitter.
- Retry only HTTP 502/503, request/connection transport failures and timeouts.
  A transient result waits for the other concurrent endpoint so its rejection
  cannot be hidden. An explicit refusal cancels outstanding provider work.
- At most 64 verification sequences per process, acquired without queuing.
  Capacity exhaustion is a local failure, never a fallback reason.
- At most 1,024 successful records per process, with a 16 KiB retained
  evidence/collection budget per record; oversized live evidence is not cached
  and removes any older record. Expire records on access/write and evict the
  oldest real verification when full. A read never changes order.
- Index records by HMAC-SHA-256 of the token with a random process-local key.
  Retain only verified evidence, verification time and expiry. Never store raw
  tokens in this fallback store, persist it, or share it between pods.
- CMA logout clears the entire local fallback store, even when durable logout
  fails, and fences pending writes. A rejection fences pending writes and
  removes its token record. Process restart creates an empty store and new key.
- Recheck browser token custody after verification/fallback and recheck the
  invalidation epoch after that asynchronous custody lookup. Fallback cannot
  borrow another token's evidence or race logout into repopulating the store.

Refresh-token exchanges retain their separate custody/lease semantics and do
not use fallback or blindly retry token rotation. A definitive `invalid_grant`
removes the access token's fallback record. Agent keys, proxy/resource/worker
credentials, independent bearers and background authorization never use
fallback, including when their URL happens to be the browser session URL.

## Fail-closed route inventory

The only allowlist entry is **browser-cookie `GET /auth/session`**. `HEAD`, other
methods, trailing paths, every `/api/` operation and future routes are denied
fallback by default. The policy test checks every operation in the shared
public contract inventory in addition to unknown routes and methods.

| Sensitive surface in this repository | Routes that remain live-only |
| --- | --- |
| Admin and Ops | All `/api/v1/admin/*`: activity, profiles and profile retirement, runtime inventory, sandbox list/detail/events/retirement/runtime updates/always-on, settings GET/PUT/PATCH/reset, trigger pause/resume, warm pools GET/POST/PUT, auto-pause preview/pause-idle-now, usage summaries, usage metrics reports/series/owners/options. |
| Account and billing/usage | All `/api/v1/usage*`, including sandbox detail/events and measured report creation/read/series; agent `usage` and `channel-usage`/history. NyxID owns account/billing mutation; CMA adds no account or billing mutation exception. |
| Organizations, permissions, roles and sharing | `GET /api/v1/organizations`; `/api/v1/resource-access/{kind}/{id}`; every resource-share, access-request, agent share-link and document share/share-link operation; profile authoring/publishing/retirement and all other permission-bearing changes. |
| Secrets, credentials and delegated authority | Agent/sandbox `service-access`; `/api/v1/service-connections*`; `/api/v1/agent-delegations/*`; `/api/v1/cmaeg/worker-credentials/classify`; bots, bot connections, provider bindings and Gateway connection-record management; the `/api/event-gateway/{*path}` portal. OAuth, logout, proxy, resource and worker interfaces keep their dedicated authentication/recovery rules and never consume fallback. |
| Deletes | Every DELETE: agents, responses, sandboxes, triggers, workspaces, resource shares, Gateway channel/interaction/connection records, CMAEG bindings/conversations, bots, event-worker profiles, agent share links/files/browser, document shares/share links/comments. |
| Remaining API operations | All API reads, mutations, runtime commands, input submission, approvals, service forwarding and new routes. No broad read-only prefix is eligible. |

## Web and public error contract

When session verification exhausts retry without eligible evidence,
`GET /auth/session` returns HTTP 503, `application/problem+json`,
`code: identity_unavailable`, `Retry-After: 3` and `Cache-Control: no-store`.
Other authentication-store failures retain `store_failed`; other API routes
retain their existing error projection. The shared public inventory and replay
corpus pin the session error response.

The web distinguishes this error from a generic CMA connection failure and says
“The identity service is temporarily unavailable.” It retries after 3, 6, 12,
24 and then 30 seconds with 0–500 ms jitter, allowing only one concurrent check.
Manual retry and focus/online events share that check. Existing authenticated
views and drafts stay mounted during temporary failures with an alert; a real
rejection clears authenticated presentation. UI continuity grants no backend
authority and does not auto-submit drafts or replay mutations.

## Observation and rollback

Each accepted fallback emits one WARN line with `event="identity_fallback"`,
`count=1`, a 96-bit prefix of the process-keyed HMAC, and a fixed reason such as
`http_503` or `timeout`. Count those events by pod/time; do not attach subjects,
email addresses, provider response bodies or raw credentials. A restart changes
hash correlation. Attempt failures also report bounded attempt numbers/reasons.

Rollback is a normal release of the prior API/web image or a revert of this PR,
coordinated by the lead through the existing serialized schema_guard deployment
process. There is no schema, persisted fallback state, migration, dependency or
cross-pod cache to unwind. Restarting the API immediately clears its local
fallback evidence. This task does not deploy or merge; an independent security
review is required before merge.
