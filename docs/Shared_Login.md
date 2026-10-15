# Shared Agent Manager and Event Gateway login

The CMA portal uses Agent Manager's existing NyxID OAuth application and opaque,
HttpOnly `cma_session` cookie for both Agent Manager and Event Gateway. The browser
starts the existing `/auth/login?return_to=/event-gateway` authorization-code/PKCE
flow and signs out through `/auth/logout`. It never receives a NyxID access or
refresh token. Image Manager keeps its independent login until its later rollout.

Agent Manager serves the public `/api/event-gateway` portal bridge. It resolves
exactly one CMA browser cookie, refreshes the stored credential with the existing
renewal lease, verifies current provider evidence, and forwards the resulting
bearer to the configured `CHRONO_CMAEG_BASE_URL`. Event Gateway independently
introspects that bearer, enforces its own NyxID resource restriction and
`cma-eg:*` permission checks, and retains all existing record ownership checks.
An Agent Manager permission does not imply an Event Gateway permission.

The bridge permits only `GET /auth/session`, `GET /openapi.json`,
`GET /v1/permissions`, `GET|POST /v1/channels`,
`GET|PUT|DELETE /v1/channels/{id}`, `POST /v1/channels/{id}/test`, and
`GET /v1/channel-records/{id}`. Paths are canonical and identifiers cannot contain
slashes or traversal. Requests and responses are bounded to one MiB. Requests
that mutate state require one exact same-origin `Origin`. Explicit bearer headers
and duplicate CMA session cookies are rejected at this cookie-only portal entry;
external bearer clients continue to use the Gateway API directly.

Only content type, accept, ETag concurrency and idempotency headers are forwarded.
Browser cookies, selected-owner, proxy-identity and hop-by-hop headers are removed.
Upstream redirects are refused, upstream cookies are withheld, and portal replies
are `no-store`. The exact browser login and credential custody are rechecked before
dispatch and before returning an upstream reply, so logout or token replacement
cannot restore a stale session view. Already dispatched writes retain the upstream
API's normal idempotency and reconciliation behavior.

Event Gateway's `/auth/session` is now a bearer-verified identity view and does not
issue an independent portal cookie or initiate OAuth. Its Deployment references
Agent Manager's existing client ID ConfigMap key and client secret Secret key.
No new OAuth application, grants, database sharing or session-record migration is
required. Historical `eg_auth_sessions` records may expire normally and are no
longer accepted by the portal. The retained separate legacy Gateway console is
outside this portal login flow.

Deploy the Agent Manager bridge and Gateway bearer-session view before routing
`/api/event-gateway` to Agent Manager without a prefix rewrite. The web nginx
fallback uses that same route. Confirm existing login, service-specific permission
refusal, token renewal, logout, account switching, and cross-origin mutation refusal.
