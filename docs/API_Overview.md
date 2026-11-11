# API Overview

See the [complete method/path inventory](CMA_Public_APIs.md) and the
[Event Gateway architecture and integration guide](CMAEG_Architecture.md).


CMA exposes one Sandbox model through public interfaces. A Sandbox contains
Workspaces, each Workspace contains Worktrees, and each Worktree has exactly one
Agent. The Agent owns the conversation; there is no separate Chat resource,
terminal plane or Session resource. The retained `/auth/session` route refers
only to browser identity.

The [CMA Model](CMA_Model.md) defines ownership, lifecycle states, profiles,
trust and recovery. The [REST contract](API_REST.md) covers every operation in
the served `/openapi.json`; protocol-specific contracts are below.

| Priority | Interface | Current purpose | Contract |
|---|---|---|---|
| 1 | AG-UI | Existing Agent turns, approval answers and replayable SSE | [AG-UI](API_AGUI.md) |
| 2 | Responses | Stateful streaming conversation projection of an Agent | [Responses](API_Responses.md) |
| 3 | REST | Profiles, Marketplace, instances, Triggers, Launches and operations | [REST](API_REST.md) |
| 4 | PTY | Absent; owned Agent terminal request has a typed refusal | [PTY](API_PTY.md) |
| 5 | MCP | Not implemented; endpoint has a typed refusal | [MCP](API_MCP.md) |
| 6 | A2A | Not implemented; endpoint and discovery have typed refusals | [A2A](API_A2A.md) |

## Identifiers And Ownership

| Identifier | Meaning |
|---|---|
| Sandbox id | Isolated runtime and pinned Sandbox Profile |
| Workspace id | Main checkout or git-initialized directory inside a Sandbox |
| Worktree id | Dedicated working directory and branch for exactly one Agent |
| Agent id | Conversation and Agent Profile snapshot |
| AG-UI `threadId` | The Agent id, not a Sandbox or Launch id |
| AG-UI response `runId` | The durable Agent turn sequence rendered as a string |
| Responses `conversation` | The same Agent id |
| Responses response id | `resp_<agent-id>_<turn-sequence>` |
| Trigger public id | Stable public entry point; publication changes its current revision |
| Launch id | One visitor's durable request to obtain their own Agent |

Treat identifiers as opaque. Names, titles, slugs and referrers are not
authorization or reuse keys. Owner routes conceal missing and foreign resources
with the same `404`. An operator capability grants only its explicitly named
administration surface, not arbitrary owner impersonation.

## Authentication And Visitors

Production uses NyxID. A backend sends a bearer credential; the same-origin web
client uses CMA's HttpOnly browser cookie. `GET /auth/login` begins OAuth,
`GET /auth/callback` finishes identity exchange, `GET /auth/session` reads the
authenticated subject and capabilities, and `GET /auth/logout` ends browser
sign-in. Do not put a bearer credential in URLs, local storage or embed markup.

`GET /auth/login?prompt=none` asks NyxID to reuse an existing provider session.
The server always accepts this protocol, including ignored `error_description`
and `error_uri` fields. The anonymous session response reports
`silent_sign_in: true` only when the strict startup boolean
`CHRONO_SILENT_SIGN_IN_ENABLED` is enabled; its code and ConfigMap defaults are
`false`. Disabling the hint does not break an already started exchange.

A valid code completes the existing browser-bound, single-use PKCE exchange.
`login_required` and other provider refusals return to the stored validated
address without authenticating. `consent_required` can start one fresh state/PKCE
exchange for NyxID consent, retaining the original logout fence and automatic
failure behavior. Discovery, token, identity and store failures of a bound
automatic attempt also return without issuing a session. Invalid/unbound states
never authenticate or trust a callback-supplied return address. Automatically
initiated states have a non-authorizing presentation prefix: when state or
binding cannot be recovered they safely return to `/` without an error page;
bound failures, including expiry before TTL cleanup, retain the validated target.
`prompt=none` with `screen=register`, other prompts and duplicate/unknown fields
are rejected. Cross-site/same-site Fetch Metadata or a foreign Origin cannot
start silent login. Non-browser clients may call these same public routes.

`GET /auth/logout` itself sets a host-only, HttpOnly, SameSite=Lax
`cma_signed_out` marker (Secure on HTTPS) for 30 days and expires `cma_session`.
This is an automatic-sign-in opt-out, not identity authority. Explicit sign-in
and registration remain available; they deliberately do not clear the opt-out.
That prevents a delayed callback response from erasing a newer logout.

Before sending the initiating-browser binding cookie, CMA reserves non-authorized
custody in `auth_browser_logins`. The callback atomically promotes only that
unexpired reservation, without upsert; logout removes the active session and
all reservations identified by its browser binding cookies. Promotion and logout
therefore serialize on the same durable record across replicas. Consent reuses
the reservation. A replay never clears a binding cookie that may still fence an
in-flight callback. Successful responses set the session before clearing the
binding; failed valid attempts revoke custody before clearing it. A failed store
revocation returns 503, still sends suppression and expires only successfully revoked cookies, retains
failed revocation handles for retry, and never
reports successful logout. No callback or navigation creates resources or submits
input. Existing sessions still require the live authorization and bounded
session-inspection policy in [Identity interruption policy](Identity_Outage_Policy.md).

The web checks once per tab, on the opening identity read or later navigation to
an eligible SPA page. Focus, visibility and online refreshes never initiate a
navigation. A stopped/failed navigation returns to the ordinary signed-out view;
the attempt marker prevents a loop. Authentication uncertainty and signed-out
presentation retain owner-scoped drafts and command keys; explicit logout or a
verified different owner clears them before that owner's view mounts.

Return paths remain canonical and relative, at most 256 bytes. The server and
both SDKs additionally preserve one `setup` query on `/bots/new` or one `tab`
query on `/sandboxes/{id}`, with the whole target bounded to 25,064 bytes. These
are the UI's existing non-authorizing query states; values remain untrusted and
are validated by the destination. Unknown/duplicate query keys are rejected so
redirect/authority parameters cannot acquire meaning across authentication.
Fragments remain excluded from automatic sign-in because they may contain share
credentials that must not be sent to the server. Public Trigger/Launch/share/
document flows keep their explicit authority-preserving sign-in. `/` returns to
`/` on both success and refusal. Browser identity/flow responses are no-store; anonymous signup-options retains its public five-minute cache.

### Silent sign-in rollout

1. Keep `CHRONO_SILENT_SIGN_IN_ENABLED=false`. Deploy the compatible API image to
   every serving replica, and deploy its web image. Record image digests, source
   SHA and config. Pin a compatible API image as the rollback baseline.
2. Verify every replica serves the new image and `silent_sign_in: false`. Allow
   pre-upgrade attempts to expire (10 minutes); an old unfenced attempt is refused
   and the person can explicitly restart sign-in. Do not enable while any older
   replica can receive a request.
3. Enable the flag in a separate operator-controlled ConfigMap rollout/restart.
   Verify `silent_sign_in: true`, then execute the
   [acceptance matrix](CMA_Silent_Sign_In_Acceptance.md). The flag advertises
   eligibility only; it grants no authority and creates no resources.
4. Roll back activation by setting the flag to `false` and restarting the API
   fleet. Keep the compatible callback/logout implementation so in-flight
   attempts finish safely. A web rollback is independent. Keep existing sessions,
   reserved-custody records and user resources; never restore a pre-fence API
   image as the live rollback target. No database migration or cleanup is needed.

`GET /auth/signup-options` is anonymous and returns exactly
`email_signup`, `social_providers`, `invite_code_required` and
`register_available`. It reports availability, not a CMA registration endpoint.
Registration takes place at the configured NyxID public origin. An unavailable
or malformed identity-provider response closes registration availability.

Visitors are callers with `LaunchTriggers` and without `CreateSandbox`.
They can launch and use/delete their own Sandboxes, but cannot directly create
Sandboxes, manage profiles, publish Triggers or use administration routes.
Capabilities, not the role's display name or slug, determine access.

The event-gateway channel-record administration surface is granted by the exact
NyxID permission `cma:gateway:records`, which maps to the
`ManageGatewayRecords` capability. It is separate from owner Sandbox and
Profile permissions.

Only explicitly anonymous discovery, signup options, public Trigger preview and
badge routes bypass owner authentication. A public Trigger URL does not make
Launch creation anonymous. Shared rate limits and origin checks still apply.
See [Operations](CMA_Operations.md) for the NyxID prerequisites.

## Profiles And Instances

Agent Profiles configure model, thinking effort, permissions, skills and system
prompt. Workspace Profiles configure repository/directory, setup and Agent
Profile references. Sandbox Profiles configure resources, provider, model
access, services, software, environment, limits and Workspace Profile references.

Profiles have editable drafts and immutable published revisions. Instance
creation pins `{ id, revision, digest, content }`; later profile changes do not
change a running instance. Foreign executable configuration requires explicit
trust over the server-provided commands digest. Marketplace visibility is not
a safety endorsement.

An Agent Profile prompt is written verbatim to `AGENTS.md` in that Agent's own
`CODEX_HOME`, without editing repository files. Repository instructions also
apply. Skills are installed in each Agent's home before first use; the pinned
bridge can discover later filesystem changes. Profile immutability is a CMA
rule, not an assumption about provider caching. Historical evidence and its
revalidation boundary are in [Sandbox Provider](Sandbox_Provider.md#8-agent-home-evidence).

## Conversation And Replay

REST creates and manages resources. AG-UI submits to an existing Agent;
Responses conversation creation resolves an Agent context from platform defaults
and optional owned identifiers. Both protocols use the same durable Agent
log and operation service. Neither creates a hidden frontend-only conversation.

The core operation service now routes input, steering, interruption and approval
answers through typed `AgentCommand` and `AgentInput` contracts. Existing v1
adapters retain their request/response shapes and predecessor rules. Text
partitions normalize without changing whitespace; canonical fingerprints include
content, control kind, predecessor conditions and metadata. Attachment references
have an immutable digest, but dispatch currently refuses them with
`agent_content_unsupported` before sending any accompanying text.

The common `AgentEvent` projection retains the original Agent log cursor and
safe message, tool, approval, delivery and lifecycle data. Native conversation
ids, approval locators and sign-in credentials are absent from this projection.
Live fragments have a separate type with no durable cursor. New command-envelope
and admission-receipt types distinguish owner, actor, source and grant revision;
they are separate ledger contracts and do not alter retained Agent records.
The additive command ledger reserves these receipts, a response identity and a
per-Agent order atomically. Its key scope combines verified authority, Agent and
command kind; identical retries return the original receipt and changed input
conflicts. No expiration index silently makes an old key reusable. A bounded
background worker discovers admitted commands, verifies retained credential custody
and current resource authority, then claims the Agent's lane and atomically binds
dispatch before sending input.
Recovery reconciles the existing binding without sending another input. The
worker separately authorizes Sandbox management before requesting revival. That
transaction fences the exact pending command, current resource policy and expiry;
accepted lifecycle work survives cancellation of the requesting worker. The
[Responses v2 contract](API_Responses.md) provides durable human admission to
personal, shared and organization-owned Agents, receipts and explicit replay.
The authenticated actor retains their own credential custody independently of
resource ownership. Current interaction policy is required at admission and
dispatch; organization resource creation and credential provisioning remain
under development.

Agent logs, Sandbox events and Launch events have independent monotonic
sequences. REST exposes JSON event pages. AG-UI exposes SSE with a durable
cursor only on the last frame projecting a committed log entry. Live text and
reasoning have no durable SSE id and cannot be recovered after disconnect.
Do not count frames as durable events or exchange cursors between resources.

Read current resource state and complete durable history when opening a view,
then follow from the acknowledged cursor. Advance only after applying a whole
durable projection. A replay connection failure is not an Agent turn failure.
Disconnecting a streaming POST does not cancel its admitted work; replay reads,
not repeated input POSTs, are the recovery path.

The Responses v1 subset has no stream-resume operation. Its response and item
reads recover retained output, while its strict predecessor fence prevents a
caller from silently forking the Agent's linear conversation. Responses v2 uses
the common command queue and explicit durable event replay; it requires an
existing conversation and does not accept a nonnull predecessor.

## Lifecycle And Availability

Sandbox lifecycle is distinct from Agent activity. Software installation
precedes credential delivery; Workspace preparation follows it. Agent readiness
and first-message admission/delivery/completion are separately recorded facts.
Do not enable input merely because one earlier provisioning step completed.

Idle Sandboxes pause when every Agent is quiet. A later admitted operation
revives the Sandbox when needed. Changing the runtime-image default applies to
new allocations. Existing live runtimes retain their recorded images across
control-plane upgrades and rollbacks; image drift does not pause or replace them.
Independently admitted pause and recovery preserve hierarchy, dirty work, Agent
homes and conversation identity through checkpoint and restore. A temporarily unavailable observation does not
prove resource death. Render recorded recovery state and keep history visible.

Each capability must be proved against the pinned runtime contract. This
release uses contract **3**. Unsupported operations are refused by name, never
accepted and dropped. Resource limits, availability and idempotency conflicts
are typed failures, not strings for clients to interpret heuristically.

## Triggers And Connections

A Trigger selects three published profile revisions and reuse policies. A
Launch creates or reuses only the visitor's own resources. GET, navigation and
OAuth return never begin work. An explicit action supplies consent; the
authenticated creation POST may start immediately when `start_if_ready` is true
and current requirements permit it. Otherwise the retained Draft is reviewed
through the separate Start operation.

Launch state, requirements and progress are public owner-scoped REST resources.
The AG-UI Launch companion is read-only and does not turn a Launch into an
Agent `threadId`. Service connection assist creates a short-lived handoff and
polls its state; a successful redirect alone is not proof of an active grant.
There is no worker-retry, pull-request or contribution workflow in this model.

The complete publishing and visitor contract, including Markdown/HTML/script
snippets, is [CMA Trigger](CMA_Trigger.md). Service scope and connection outcomes
are documented in [Services](Services.md).

## Client Rules

- Use the deployed OpenAPI document and its typed request/response schemas.
- Preserve a mutation's idempotency key and exact body until its result settles.
- Handle `application/problem+json` by status and `code`; retain request ids for support.
- Respect capability, quota and availability refusals. Do not invent alternate private routes.
- Render unknown optional fields safely; reject identity or cursor contradictions.
- Keep credentials out of logs, errors, profile content, markup and browser state.

For v1 Agent input and control commands, a failure while looking up a prior
admission, before this request attempts admission, is saved as the acquired
operation's replay receipt. The same key and exact body replay that failure,
including a `503`, after the dependency recovers. A failed lookup does not prove
that no historical command exists: use a new key only after durable state
confirms the original intent was not admitted. An unchanged transcript alone is
insufficient because accepted work may still be queued. Older incomplete claims
remain fenced; their two-minute execution lease becoming
`idempotency_recovery_required` does not authorize a new execution or delete the
claim. Errors after admission begins retain the existing receipt-recovery path.

The repository-only [TypeScript SDK](../sdk/typescript/docs/README.md) and
[Rust SDK](../sdk/rust/docs/README.md) share the public boundary. SDK documentation
declares each package's implemented subset; neither package is registry-published.

Event ingress and channel delivery belong to the separate
[CMA Event Gateway](CMAEG_Protocol.md). CMA owns provider bindings, environments
and Agent conversations; the former in-CMA Bridge execution pipeline is retired.
