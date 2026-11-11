# Direct user OAuth command authorization

Third-party OAuth clients request `openid roles offline_access proxy` and the
provider's canonical CMA RFC 8707 resource. Business requests go directly to
CMA's public API, with the user access bearer, stable request bodies and
idempotency keys. The resource URI names the service; it does not require
routing those requests through the NyxID HTTP proxy.

Configure `CHRONO_NYXID_OAUTH_API_RESOURCE` on every CMA API replica to the exact
canonical resource returned by provider introspection. The checked-in production
value is `https://api.nyxid.ai/api/v1/proxy/s/cma`. This is a deployment audience,
not a client allowlist. No Metaverse client IDs or credentials are added to CMA
or NyxID. Unconfigured readers refuse restricted third-party command custody.

The JWT filter admits only ordinary personal user access-token shapes. Neutral
empty service/node arrays with explicit allow-all flags preserve ordinary user
authority. Restricted service grants require a nonempty matching resource,
`proxy` scope, a nonempty service list, and unrestricted node metadata. The exact
bearer must then pass live provider userinfo and authenticated introspection;
restricted grants require a positive matching resource in that response. Empty,
missing, unrelated, malformed or identity-only resource evidence cannot supply
command authority. Subject, RBAC, expiry, credential custody, resource policy,
revocation, execution and revival checks still apply to every command.

`GET /auth/session` adds `api_commands`, a nonsecret indication that the current
verified independent bearer can supply command custody and has Sandbox-use
permission. It is not a promise of access to every Agent, does not represent
browser cookie command custody, and never replaces command-time revalidation.
Clients should distinguish identity-only success from readiness to chat. Older
readers omit this field; third-party clients must not report chat readiness from
`authenticated:true` alone.

New Sandbox creation still needs the owner's renewable CMA login/delegation.
OAuth bearer command access does not establish or borrow this credential.

Production probes on 2026-11-12 found both an identity-only third-party grant and
an explicitly CMA-scoped grant could read `/auth/session`, but Responses v2
returned `403 command_authority_required`. First-party CLI OAuth could complete
two consecutive turns. Temporary probe OAuth clients were deleted afterwards.
These probes establish the existing compatibility failure; local regression
success is not evidence this reader has been deployed. Promote the tested CMA
reader/configuration before updating third-party clients, then validate their
actual grants, consecutive turns, refresh and original-request replay.
