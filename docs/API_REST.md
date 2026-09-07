# REST Interface

This is the public HTTP contract for CMA's Sandbox model. The served
`GET /openapi.json` is OpenAPI 3.1 and supplies exact JSON schemas, parameters,
security declarations and response media types. The operation inventory below
is checked against that document, including its AG-UI and Responses projections.
[CMA Model](CMA_Model.md) defines lifecycle and profile semantics;
[CMA Trigger](CMA_Trigger.md) defines publishing, consent and visitor reuse.

There are no `/sessions`, `/chats`, environment-profile, contribution,
sponsorship or terminal-attachment APIs in this release. Resource management
uses Sandbox, Workspace, Worktree and Agent ids. A resource's readable name is
not an id or a concurrency fence.

## Authentication And HTTP

Use `Authorization: Bearer <credential>` for backend calls, or the same-origin
CMA browser cookie. Never embed credentials in query strings, HTML or badges.
Cookie requests are subject to origin defenses; cross-origin browser clients
must use an explicitly configured CORS origin. Allowed request headers include
`Content-Type`, `Authorization`, `Last-Event-ID`, `Idempotency-Key` and
`X-Request-Id`; exposed response headers include `Link`, `Retry-After` and
`X-Request-Id`. CORS is not authorization.

Creating a new Sandbox also requires renewable owner delegation already held by
CMA. A request bearer authenticates that request; CMA does not persist it or
derive refresh authority from it. Complete the existing `/auth/login` OAuth
flow with the same account to establish delegation. Bearer SDK calls remain
supported after this setup; the browser session need not accompany them.

`POST /api/v1/sandboxes`, public Launch creation, and Draft Launch start return
`409 sandbox_credential_setup_required` when a new Sandbox is needed but that
delegation is missing or no longer renewable. No Sandbox is admitted, and a
refused Draft start leaves the Launch unchanged. Reused Sandboxes and Agents do
not acquire this prerequisite merely for reuse. Identity-store or provider
outages remain sanitized `503` failures, not a request to repeat sign-in.
Default-context `POST /api/v1/conversations` inherits this prerequisite when it
needs a new Sandbox. That protocol-compatible operation is unkeyed: retry only
a definitive setup refusal after sign-in, not an ambiguous creation failure.

Use the SDK's `auth.loginUrl` or `auth().login_url` with a canonical CMA return
path, preserve the pending command without any credential, and verify that the
returned identity is the same subject. A successful browser session alone does
not prove the provider supplied renewable delegation: the next admission still
checks it. For keyed operations, after a definitive setup-required refusal and
successful sign-in, explicitly retry with a **new** idempotency key. The previous
key continues to replay its exact refusal; transport-ambiguous retries must
retain the original key and body. Neither OAuth callback nor return navigation
starts work.

Send `Content-Type: application/json` when a route has a JSON body. An empty
object and an absent body are not interchangeable: Agent stop requires `{}`
(or its documented reason field), whereas interrupt, pause and revive have no
request schema. Request bodies are capped at 2 MiB before per-field validation.
Unknown request fields are refused on closed DTOs. Integers, enums and profile
references are validated, not coerced from arbitrary strings.

Owner routes return the same `404` for missing and foreign resources. An
administrator uses the capability-gated `/admin` operations rather than
impersonating another owner. Public preview and badge access does not authorize
Launch creation. Visit [API Overview](API_Overview.md#authentication-and-visitors)
for browser identity and visitor capabilities.

The composed identity/discovery/static routes also include:

| Operation | Meaning |
|---|---|
| `GET /openapi.json` | Anonymous machine-readable contract |
| `GET /auth/login` | Begin NyxID OAuth; return destination is validated |
| `GET /auth/callback` | Complete identity exchange; never starts Agent work |
| `GET /auth/session` | Browser identity, capabilities and visitor flag; 401 when signed out |
| `POST /auth/logout` | Revoke browser login and expire cookie; redirect to `/` |
| `GET /embed/v1.js` | Fixed versioned-by-integrity embed script; no credential access |
| `POST /api/v1/mcp` | Typed 501 `interface_not_available` |
| `GET /.well-known/agent-card.json` | Typed 501 `interface_not_available` for A2A |
| `/api/v1/a2a` | Typed 501 `interface_not_available` |

`GET /auth/signup-options` appears in the checked inventory. Its four-field
response is exactly `{ email_signup, social_providers, invite_code_required,
register_available }`. CMA reports registration availability but never creates
NyxID users or roles. Anonymous preview/badge GETs and OAuth callbacks are inert.

## Errors, Retry And Concurrency

Errors use `application/problem+json` with standard `type`, `title`, `status`,
`detail` and a stable `code`. Additional structured context may name a resource,
limit, current revision, required digest, cursor head or reset time. Inspect
`code` and HTTP status, not prose. Preserve `X-Request-Id` for support without
logging credentials or rejected secret-bearing input.

| Status | Typical handling |
|---|---|
| 400 | Correct malformed body, query, cursor or idempotency header |
| 401 | Reauthenticate; do not retry an unauthenticated mutation blindly |
| 403 | Capability refusal; changing the URL or resource id cannot grant access |
| 404 | Missing or concealed resource; do not infer another owner's existence |
| 409 | State, draft-version, expected-revision, trust or idempotency conflict; read current state |
| 422 | Unsupported operation or semantic validation refusal; correct input |
| 429 | Count/rate limit; respect the reset information and `Retry-After` when supplied |
| 503 | Dependency or durable-store unavailable; retry only within the operation's safety contract |

Every inventory row marked `key` requires `Idempotency-Key`. Generate a fresh
opaque key per logical mutation, retain its exact body, and reuse that pair when
the result is ambiguous. Reusing a key with changed content conflicts. An exact
replay returns the original status/body, not a fresh snapshot. In particular,
replaying a Draft Launch's creation key still returns its original 201 even
after the Launch has progressed; read the Launch to obtain current state.

`idempotency_in_flight` and `idempotency_recovery_required` leave the command
unsettled. Do not replace its key merely because a response was lost. Read-only
GET retries and SSE reconnects do not authorize a new input or create POST.
AG-UI run and Responses creation do not use this REST key contract: submit
once and recover retained output through reads.

Profiles use `draft_version` for draft save/publication. Settings use
`expected_revision`. These are server-issued optimistic concurrency values,
not timestamps or request ids. A conflict requires a new read and an explicit
reconciliation of the user's intended changes; never overwrite newer state
using an automatically substituted revision.

## Profile Authoring

The four `/api/v1/profile-authoring` operations require authentication and
`ManageOwnProfiles`. They are read-only lookups, including the repository
resolver's POST: no idempotency key, resource admission, profile edit, clone,
or Sandbox creation occurs. All successful responses are HTTP 200. Optional
output fields are omitted rather than returned as `null`.
Service and skill lookups use the current request's delegated authority when
present, including for bearer callers without a previous browser sign-in.
That authority is transient, never persisted by the lookup, and never included
in a public response or repository verification request.

`GET /api/v1/profile-authoring/bounds` returns
`{ min_cpu_millis, max_cpu_millis, min_memory_mib, max_memory_mib }` from the
same fixed deployment policy used to validate Sandbox Profiles. It does not
depend on catalogue or repository availability.

`GET /api/v1/profile-authoring/services` returns an array of owner-visible
`{ id, name, description? }` choices. An empty array is a successful empty
catalogue. Source failures return `503 profile_services_unavailable`, not an
empty catalogue or credentials.
Catalogue descriptions retain the complete source text up to 6144 UTF-8 bytes;
this authoring limit does not change the separate profile draft limits.

`GET /api/v1/profile-authoring/skills?q=&limit=&cursor=` returns
`{ items: [{ id, name, description?, versions: [string] }], next? }`. Omitted
`q` browses with an empty query; queries allow at most 200 UTF-8 bytes and no
control characters. The limit defaults to 20 and accepts 1-50. A supplied
cursor must be nonempty, at most 2048 bytes, and free of control characters.
Versions are up to 100 latest selectable, nondeprecated releases, ordered
newest-first by numeric `major.minor` and retained as strings. Existing profile
pins are not changed by catalogue filtering. A filtered page may contain no
items while retaining an advancing `next`; continue until `next` is absent.
Invalid queries and source-rejected cursors return
`400 invalid_profile_authoring_query`; unavailable or malformed source data
returns `503 profile_skills_unavailable`.

`POST /api/v1/profile-authoring/repositories/resolve` takes `{ url, reference? }`
for a public GitHub repository and an optional branch or tag. The URL must
contain exactly two nonempty ASCII owner/repository components,
each at most 100 bytes after removing the repository's optional `.git` suffix.
A single trailing slash is accepted. The response is
`{ url, reference?, default_branch, commit_sha }`. The URL is canonical and may
normalize owner/repository case and remove `.git` or trailing slashes.
`reference` is retained exactly when requested and omitted otherwise; the
observed default branch is never substituted into it. `default_branch` and the
lowercase 40-hex `commit_sha` are verified server-side observations. No caller
credential is forwarded to GitHub and redirects are not followed.

Invalid URLs or references return `422 invalid_repository` or
`invalid_repository_reference`; repository absence/inaccessibility returns
`404 repository_not_found`, and a missing branch/tag returns
`404 repository_reference_not_found`. Failed verification returns
`503 repository_unavailable`. Malformed JSON, incompatible body fields,
oversized bodies, and missing/wrong content types use `invalid_json` with
400, 422, 413, and 415 respectively. All four routes return
`401 not_authenticated` or `403 forbidden` before exposing validation or source
results to unauthorized callers.

## Profiles And Marketplace

The three profile APIs share lifecycle shape but have different content schemas.
Create uses `{ name, description?, content }`; read returns identity, owner,
visibility, draft content, `draft_version` and publication information. Update
uses `{ draft_version, content, name?, description? }`. Publication posts
`{ draft_version }` to `/revisions` and returns an immutable published revision.
Visibility uses `{ visibility: "private" | "public" }`. Making a profile private
or retiring it hides discovery without invalidating existing instance snapshots
or exact published references. A retired profile cannot publish new revisions.

A profile reference is `{ "id": "agp_example", "revision": 1 }`, with a
positive published revision of the correct kind. Agent, Workspace and Sandbox
references are not interchangeable. Array order is preserved and the canonical
content digest includes that order. The exact reserved defaults `agp_default`
and `wkp_default` are accepted as Agent and Workspace references respectively;
other reserved-looking strings and cross-kind IDs are not interchangeable.

Newly selected references must resolve to an owned or public, nonretired profile
revision. An exact pin inherited from the server-recorded fork source or the
parent's own prior published revisions remains usable when the child becomes
private or retired. Changing its ID or revision is a new selection and requires
current access. Draft JSON alone never grants reference authority, and inherited
pins do not grant direct access to a private child's profile routes. Publication
rechecks these rules atomically with reading the draft and inserting its revision.

```json
{
  "name": "Repository reviewer",
  "description": "Read and review repository changes",
  "content": {
    "model": null,
    "thinking_effort": "medium",
    "permission_level": "guarded",
    "skills": [],
    "system_prompt": "Review the repository and report actionable findings."
  }
}
```

Workspace content has `location`, `name`, `setup_commands` and
`agent_profiles`. `location` is `{ type: "git_repository", url, reference? }`
or `{ type: "directory", name }`. Repositories must be supported public GitHub
URLs. The effective directory is under `/home/cma`; main checkouts are never
Agent working directories.

Sandbox content has `cpu_millis`, `memory_mib`, `nyxid_services`,
`agent_provider`, `model_access`, `max_workspaces`, `max_agents_per_workspace`,
`workspace_profiles`, `software_install_commands` and `environment_variables`.
Only `agent_provider: "codex"` is supported. Model access is
`chronoai_managed` or `own_credentials`. Environment variables are public
configuration, not secret storage. [CMA Model](CMA_Model.md#profile-contract)
lists complete field bounds, reserved names and trust rules.

Marketplace `kind` is `agent`, `workspace` or `sandbox`, not the plural profile
route component. Listing accepts search `q` and a cursor. Detail exposes usable
published content and transitive trust disclosure. Fork posts `{}` for the
current usable revision or `{ revision: n }` for an explicit revision, returning
a new caller-owned profile with provenance. There is no shared mutable fork.

## Instance Creation And Lifecycle

`GET /api/v1/defaults` returns three exact published references named
`sandbox_profile`, `workspace_profile` and `agent_profile`. Do not guess default
ids, reconstruct references from display names or silently use a later revision.

Create a Sandbox with `{ sandbox_profile: { id, revision }, name? }`.
Create a Workspace under it with `{ workspace_profile: { id, revision } }`.
Both requests accept `trust_confirmed` and `commands_digest` when the selected
foreign executable content requires explicit review. Use the server-provided
digest; trust for one command set does not authorize another.

Create an Agent under a ready Workspace with:

```json
{
  "agent_profile": { "id": "agp_example", "revision": 1 },
  "title": "Repository review",
  "first_message": "Inspect the repository and summarize the changes."
}
```

The response contains the Agent and its parent context. Worktree and Agent
admission reserves one dedicated branch/directory atomically; there is no
independent Worktree-create route. Creation acceptance is not readiness or
first-message completion. Follow the Agent, Sandbox events and recorded
first-message state. Counts are enforced transactionally at admission.

Sandbox pause/revive and removal return recorded lifecycle state. Stopping an
Agent preserves readable history; deleting it records cleanup of its Worktree
and home. Workspace removal and Sandbox retirement are durable operations,
not a direct recursive filesystem API. The [CMA Model](CMA_Model.md#lifecycle-states)
contains the actual transition whitelist.

## Agent Input And Reads

REST input and steer accept `{ text }`. Input returns `202` with `{ queued }`;
queueing is an admission fact, not proof of runtime delivery. Approval answers
use `{ decision: "allow" | "deny" | "abort" }`. Stop accepts `{}` or
`{ reason: "owner" }`; a caller cannot impersonate an internal failure reason.
Interrupt, approval, stop and deletion return empty 202 bodies where indicated.
Do not parse those bodies as JSON or infer completion from acceptance alone.

The Agent detail includes its Sandbox, Workspace, Worktree, profile, status
and first-message information, not an aggregate revision field. Transcript returns `{ agent_id,
through_seq, turns }`; usage has its own `through_seq` and cumulative input,
output and context readings. Resource readings are Sandbox-scoped observations
projected alongside Agent identity, not per-Agent billing.

`GET /api/v1/agents/{id}/events` is a **JSON page**, not SSE. It accepts `after`
and `limit` (1-200, default 200), returns `items` and optional `next`, and omits
internal dispatch keys and runtime approval locators. Read all pages, require
advancing cursors and combine transcript fallback without duplicating messages.

Each transcript turn includes ordered `messages` with the durable `sequence`,
`role` (`user` or `assistant`), `content` and `appended_at`. Steering remains a
distinct owner message inside the active turn, even when assistant messages
precede it. The `owner_message` summary holds only the initial input and
`agent_messages` only assistant text; use `messages` to reconstruct chronology.
Merge transcript and event replay by durable message sequence, not by turn id.
Sandbox events are separately scoped JSON history. Launch event pages have
`launch_id`, `events` and optional `next_cursor`; their cursors must not be used for Agent history.
For live Agent and Launch streams see [AG-UI](API_AGUI.md).

## Triggers, Launches And Connections

[CMA Trigger](CMA_Trigger.md) gives complete publisher content and fixture-derived
request examples, source placement rules, the a/b/c reuse mapping, consent,
requirements and all five snippet forms. Do not infer the Launch body from a
preview or reuse an authoring DTO for visitor admission.

The anonymous preview is a safe projection, not the publisher-owned draft.
Create Launch is authenticated and takes a stable idempotency key. It can
return a reviewed Draft (201) or admitted work (202) when `start_if_ready` and
current requirements allow it. A retained Draft continues through
`POST /api/v1/launches/{id}/start`, which rechecks requirements and explicit
trust/repeat consent. Cancellation is separate. There is no public worker retry.

The requirements snapshot includes `services`. Each entry keeps its service id
and state and may additionally expose current `display_name`,
`connection_mode: "hosted" | "manual" | "unknown"` and `setup_url`. Missing
metadata remains unknown; a slug is not evidence that SSH/manual setup is
needed. Historical Launch snapshots are not rewritten by current metadata.

Connection assist creates a short-lived owner-bound handoff for one service
and reads its status. It never puts a service credential in the response.
Only an active service recheck can establish connection; popup close, OAuth
return and a persisted handoff id cannot. [Services](Services.md) documents
hosted/manual/unsupported/unavailable outcomes and recovery.

The embed descriptor is owner-only and requires `placement_id`. It provides
server-derived URLs, snippets, SRI and CSP directives. Public badges are
anonymous SVG with closed label/theme query values, optional placement
attribution, ETag and conditional 304 handling. Never supply arbitrary SVG,
HTML, style, redirect origins or credential-bearing query parameters.

## Administration

Administration exposes settings, Sandbox/profile/runtime inventories, activity,
retirement and Trigger pause/resume under specific operator capabilities.
Owner views do not gain these operations from a route name or role slug.
Settings replacement uses `{ expected_revision, settings }`; patch uses
`expected_revision` plus complete changed sections, not arbitrary nested
JSON Patch operations. Reset uses `{ expected_revision }`. Read the resulting
revision after every successful change. Runtime inventory can fail unavailable
without proving that a runtime is absent.

See [Operations](CMA_Operations.md) for settings meaning, deployment prerequisites,
alerts, recovery, data-reset boundaries and production evidence. There is no
arbitrary shell, database deletion or NyxID administration endpoint here.

## Operation Inventory

The following rows are derived from the actual OpenAPI export and checked for
complete method/path coverage. Schema names refer to
`/openapi.json#/components/schemas/<name>`. `key` means the required
`Idempotency-Key` header. Path variables are required; listed query fields are
optional unless marked `*`. Success schemas describe the projected response,
not a guarantee that asynchronous work has finished. The shared error contract
above applies even when an operation annotates only its principal responses.

<!-- openapi-operations:start -->
### Administration

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/admin/activity` | - | actor, action, limit, cursor | 200 ActivityPageView |
| `GET /api/v1/admin/profiles` | - | kind, visibility, owner, cursor, limit | 200 ProfilePageView |
| `POST /api/v1/admin/profiles/{kind}/{id}/retire` | - | - | 200 ProfileSummaryView |
| `GET /api/v1/admin/runtime-inventory` | - | - | 200 RuntimeInventoryView |
| `GET /api/v1/admin/sandboxes` | - | owner, status, runtime_version, cursor, limit | 200 SandboxPageView |
| `GET /api/v1/admin/sandboxes/{id}` | - | - | 200 SandboxDetailView |
| `GET /api/v1/admin/sandboxes/{id}/events` | - | after, limit | 200 array<object> |
| `POST /api/v1/admin/sandboxes/{id}/retire` | - | - | 200 object |
| `GET /api/v1/admin/settings` | - | - | 200 SettingsView |
| `PUT /api/v1/admin/settings` | ReplaceRequest | - | 200 SettingsView |
| `PATCH /api/v1/admin/settings` | PatchRequest | - | 200 SettingsView |
| `POST /api/v1/admin/settings/reset` | ResetRequest | - | 200 SettingsView |
| `POST /api/v1/admin/triggers/pause` | - | - | 200 SettingsView |
| `POST /api/v1/admin/triggers/resume` | - | - | 200 SettingsView |

### Agent profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agent-profiles` | - | cursor, limit | 200 AgentProfilePageView |
| `POST /api/v1/agent-profiles` | CreateAgentProfileRequest; key | - | 201 AgentProfileView |
| `GET /api/v1/agent-profiles/{id}` | - | - | 200 AgentProfileView |
| `PUT /api/v1/agent-profiles/{id}` | UpdateAgentProfileRequest; key | - | 200 AgentProfileView |
| `POST /api/v1/agent-profiles/{id}/retire` | -; key | - | 200 AgentProfileView |
| `GET /api/v1/agent-profiles/{id}/revisions` | - | cursor, limit | 200 AgentProfileRevisionPageView |
| `POST /api/v1/agent-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 AgentProfileRevisionView |
| `GET /api/v1/agent-profiles/{id}/revisions/{revision}` | - | - | 200 AgentProfileRevisionView |
| `PUT /api/v1/agent-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 AgentProfileView |

### Agents

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents` | - | sandbox, workspace, status, cursor, limit | 200 AgentPageView |
| `GET /api/v1/agents/{id}` | - | - | 200 AgentView |
| `DELETE /api/v1/agents/{id}` | -; key | - | 202 empty |
| `GET /api/v1/agents/{id}/events` | - | after, limit | 200 AgentEventPageView |
| `POST /api/v1/agents/{id}/stop` | StopAgentRequest; key | - | 202 empty |
| `GET /api/v1/agents/{id}/terminal` | - | - | 404 ProblemDocument; 501 ProblemDocument |
| `GET /api/v1/agents/{id}/transcript` | - | - | 200 AgentTranscriptView |
| `POST /api/v1/workspaces/{id}/agents` | CreateAgentRequest; key | - | 201 AgentView |

### Agent operations

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/agents/{id}/approvals/{request_id}` | ApprovalDecisionRequest; key | - | 202 empty |
| `POST /api/v1/agents/{id}/input` | AgentTextRequest; key | - | 202 InputAcceptedView |
| `POST /api/v1/agents/{id}/interrupt` | -; key | - | 202 empty |
| `POST /api/v1/agents/{id}/steer` | AgentTextRequest; key | - | 202 empty |

### Agent usage

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/resources` | - | - | 200 AgentResourcesView |
| `GET /api/v1/agents/{id}/usage` | - | - | 200 AgentUsageView |

### Ag-ui

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agui/agents/{id}/sandbox` | - | - | 200 JSON |
| `GET /api/v1/agui/capabilities` | - | - | 200 JSON |
| `GET /api/v1/agui/events` | - | thread_id*, starting_after, after, follow | 200 SSE |
| `POST /api/v1/agui/run` | RunRequest | - | 200 SSE |

### Trigger launches

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agui/launches/{id}/events` | - | starting_after, after, limit, follow | 200 SSE |
| `GET /api/v1/launches` | - | trigger, state, cursor, limit | 200 LaunchPageView |
| `GET /api/v1/launches/{id}` | - | - | 200 LaunchView |
| `POST /api/v1/launches/{id}/cancel` | -; key | - | 200 LaunchView |
| `GET /api/v1/launches/{id}/events` | - | after, limit | 200 LaunchEventPageView |
| `GET /api/v1/launches/{id}/requirements` | - | - | 200 RequirementsSnapshotView |
| `POST /api/v1/launches/{id}/start` | StartLaunchRequest; key | - | 202 LaunchView |
| `POST /api/v1/triggers/public/{public_id}/launches` | CreateLaunchRequest; key | - | 201 LaunchView; 202 LaunchView |
| `POST /api/v1/triggers/public/{public_id}/entry-intent` | - | - | 200 TriggerEntryIntentView |

### Discovery

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/conformance` | - | - | 200 ConformanceDocument |
| `GET /auth/signup-options` | - | - | 200 SignupOptions |

### Responses

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/conversations` | CreateConversationRequest | - | 200 Conversation |
| `GET /api/v1/conversations/{id}` | - | - | 200 Conversation |
| `GET /api/v1/conversations/{id}/items` | - | after, limit | 200 ConversationItemList |
| `POST /api/v1/responses` | CreateResponseRequest | - | 200 SSE |
| `GET /api/v1/responses/{response_id}` | - | - | 200 ResponseObject |
| `DELETE /api/v1/responses/{response_id}` | - | - | 422 ProblemDocument |
| `POST /api/v1/responses/{response_id}/cancel` | - | - | 422 ProblemDocument |

### Profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/defaults` | - | - | 200 DefaultProfilesView |

### Profile authoring

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/profile-authoring/bounds` | - | - | 200 ProfileCapacityBoundsView |
| `GET /api/v1/profile-authoring/services` | - | - | 200 array<AuthoringServiceView> |
| `GET /api/v1/profile-authoring/skills` | - | q, limit, cursor | 200 AuthoringSkillPage |
| `POST /api/v1/profile-authoring/repositories/resolve` | ResolveRepositoryRequest | - | 200 RepositoryResolutionView |

### Profile marketplace

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/marketplace/{kind}` | - | q, cursor, limit | 200 MarketplacePageView |
| `GET /api/v1/marketplace/{kind}/{id}` | - | - | 200 MarketplaceEntryView |
| `POST /api/v1/marketplace/{kind}/{id}/fork` | ForkProfileRequest; key | - | 201 ForkedProfileView |

### Sandbox profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandbox-profiles` | - | cursor, limit | 200 SandboxProfilePageView |
| `POST /api/v1/sandbox-profiles` | CreateSandboxProfileRequest; key | - | 201 SandboxProfileView |
| `GET /api/v1/sandbox-profiles/{id}` | - | - | 200 SandboxProfileView |
| `PUT /api/v1/sandbox-profiles/{id}` | UpdateSandboxProfileRequest; key | - | 200 SandboxProfileView |
| `POST /api/v1/sandbox-profiles/{id}/retire` | -; key | - | 200 SandboxProfileView |
| `GET /api/v1/sandbox-profiles/{id}/revisions` | - | cursor, limit | 200 SandboxProfileRevisionPageView |
| `POST /api/v1/sandbox-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 SandboxProfileRevisionView |
| `GET /api/v1/sandbox-profiles/{id}/revisions/{revision}` | - | - | 200 SandboxProfileRevisionView |
| `PUT /api/v1/sandbox-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 SandboxProfileView |

### Sandboxes

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandboxes` | - | status, cursor, limit | 200 OwnerSandboxPageView |
| `POST /api/v1/sandboxes` | CreateSandboxRequest; key | - | 201 SandboxView |
| `GET /api/v1/sandboxes/{id}` | - | - | 200 OwnerSandboxDetailView |
| `DELETE /api/v1/sandboxes/{id}` | -; key | - | 202 SandboxView |
| `GET /api/v1/sandboxes/{id}/events` | - | after, limit | 200 array<SandboxEventView> |
| `POST /api/v1/sandboxes/{id}/pause` | -; key | - | 202 SandboxView |
| `POST /api/v1/sandboxes/{id}/revive` | -; key | - | 202 SandboxView |

### Workspaces

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandboxes/{id}/workspaces` | - | - | 200 WorkspaceListView |
| `POST /api/v1/sandboxes/{id}/workspaces` | CreateWorkspaceRequest; key | - | 201 WorkspaceView |
| `GET /api/v1/workspaces/{id}` | - | - | 200 WorkspaceDetailView |
| `DELETE /api/v1/workspaces/{id}` | -; key | - | 202 WorkspaceView |

### Service connections

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/service-connections` | CreateServiceConnectionRequest; key | - | 201 CreatedServiceConnectionView |
| `GET /api/v1/service-connections/{handoff_id}` | - | - | 200 ServiceConnectionStatusView |

### Triggers

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/triggers` | - | cursor, limit | 200 TriggerPageView |
| `POST /api/v1/triggers` | CreateTriggerRequest; key | - | 201 TriggerView |
| `GET /api/v1/triggers/public/{public_id}` | - | - | 200 TriggerPreviewView |
| `GET /api/v1/triggers/public/{public_id}/badge.svg` | - | label, theme, placement | 200 SVG; 304 empty |
| `GET /api/v1/triggers/public/{public_id}/open/{placement_id}` | - | - | 303 public Trigger page |
| `GET /api/v1/triggers/{id}` | - | - | 200 TriggerView |
| `PUT /api/v1/triggers/{id}` | UpdateTriggerRequest; key | - | 200 TriggerView |
| `DELETE /api/v1/triggers/{id}` | -; key | - | 204 empty |
| `POST /api/v1/triggers/{id}/publish` | PublishTriggerRequest; key | - | 201 PublishedTriggerRevisionView |
| `POST /api/v1/triggers/{id}/retire` | -; key | - | 200 TriggerView |
| `GET /api/v1/triggers/{id}/revisions` | - | - | 200 array<TriggerRevisionView> |
| `GET /api/v1/triggers/{id}/stats` | - | - | 200 TriggerStatisticsView |

### Trigger placements

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/triggers/{id}/embed` | - | placement_id* | 200 EmbedDescriptor |
| `GET /api/v1/triggers/{id}/placements` | - | include_retired | 200 array<PlacementView> |
| `POST /api/v1/triggers/{id}/placements` | CreatePlacementRequest; key | - | 201 PlacementView |
| `POST /api/v1/triggers/{id}/placements/{placement_id}/retire` | -; key | - | 200 PlacementView |

### Workspace profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/workspace-profiles` | - | cursor, limit | 200 WorkspaceProfilePageView |
| `POST /api/v1/workspace-profiles` | CreateWorkspaceProfileRequest; key | - | 201 WorkspaceProfileView |
| `GET /api/v1/workspace-profiles/{id}` | - | - | 200 WorkspaceProfileView |
| `PUT /api/v1/workspace-profiles/{id}` | UpdateWorkspaceProfileRequest; key | - | 200 WorkspaceProfileView |
| `POST /api/v1/workspace-profiles/{id}/retire` | -; key | - | 200 WorkspaceProfileView |
| `GET /api/v1/workspace-profiles/{id}/revisions` | - | cursor, limit | 200 WorkspaceProfileRevisionPageView |
| `POST /api/v1/workspace-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 WorkspaceProfileRevisionView |
| `GET /api/v1/workspace-profiles/{id}/revisions/{revision}` | - | - | 200 WorkspaceProfileRevisionView |
| `PUT /api/v1/workspace-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 WorkspaceProfileView |

### Worktrees

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/workspaces/{id}/worktrees` | - | - | 200 WorktreeListView |
| `GET /api/v1/worktrees/{id}` | - | - | 200 WorktreeView |

<!-- openapi-operations:end -->

## Contract Verification

From the repository root, export the actual Rust OpenAPI document and pass it
to the route-coverage checker (set `CARGO_TARGET_DIR` to the shared target when
working in this milestone's coordinated worktree):

```sh
cargo run --manifest-path api/Cargo.toml -p layer1-public --example openapi --locked --quiet | node .github/scripts/check-api-docs.mjs
node --test .github/scripts/check-api-docs.test.mjs
```

The Layer 1 test suite independently checks that every exported method/path is
present in this document, so route additions cannot silently pass the backend
gates while leaving this inventory stale. The OpenAPI JSON remains a generated
artifact, not a checked-in substitute for the server's current contract.
