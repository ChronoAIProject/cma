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

`GET /auth/login?prompt=none` asks NyxID to reuse an existing provider session
without showing anything. NyxID returns a code when the browser is signed in;
otherwise the callback redirects to the validated `return_to` without a cookie,
instead of reporting a failed sign-in. `prompt=none` cannot be combined with
`screen=register`, and no other `prompt` value is accepted. The web client tries
this once per tab before showing the signed-out landing, only on pages that are
valid return targets, and not after the person has signed out of CMA until they
choose to sign in again: CMA logout ends the CMA cookie, not the NyxID session.

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
