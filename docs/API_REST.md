# REST Interface

**Priority 2 — for backend server clients.**

REST is the **superset interface**: it projects all nine core operations and is the reference realisation of the core session model. Every other interface is a projection of the same model; where a question about semantics is ambiguous elsewhere, this document is the answer.

See [API_Overview.md](./API_Overview.md) for the core operation set, session lifecycle, and event model.

---

## 1. Conventions

### Authentication

Two credentials are accepted, because there are two kinds of client.

A **browser** signs in through the identity provider and is given a session cookie. Send it with every request (`credentials: 'include'` from `fetch`).

A **server** presents a bearer token issued by the identity provider:

```http
Authorization: Bearer <access token>
```

This is the form that CMA (ChronoAI Managed Agent) clients use. A server has no cookie jar, no redirect to follow and no browser to send to a login page, so the cookie is not available to it.

The token is checked with the provider rather than decoded here, which is what makes a withdrawn token stop working. The answer is cached for up to a minute, so revocation takes effect within that window rather than immediately.

Capabilities are the same either way: they come from the subject's roles as the provider reports them, so a client is never more privileged for having arrived over one route than the other.

A request with neither credential is `401`. A request whose credential is valid but insufficient is `403`.
The unauthenticated response carries `code: "not_authenticated"`; clients branch
on that code and the status, never on its prose.


| Concern | Rule |
|---|---|
| Base path | `/api/v1` |
| Auth | NyxID bearer token, or the CMA session cookie for browser clients |
| Errors | RFC 9457 `application/problem+json` |
| Rate limiting | RFC 6585 `429` with RFC 9110 `Retry-After` |
| Pagination | RFC 8288 `Link` with `rel="next"` |
| Idempotency | **Not implemented.** No route reads `Idempotency-Key`; sending one is harmless and does nothing. This row previously said it was required on every side-effecting `POST`, which was never true — §10 asks for an absence to be declared rather than discovered |
| Timestamps | RFC 3339, UTC, always `Z` |
| Versioning | Additive within `/v1`. A breaking change mints `/v2`; `/v1` is never redefined |

Every response carries `X-Request-Id`, including authentication failures and
unmatched routes. A supplied value is echoed; otherwise CMA mints a UUID. The
same value is attached to the request's tracing span. Include it in any support
report.

---

## 2. Environment Profiles

An environment profile is the named, reusable description from which sessions
are created: variables, software, runtime and model choices, resource size,
skills, service reach, and the owner's standing instructions. A session request
selects the profile by `id`; it does not repeat these fields.

Profiles are private to their owner. Collection reads contain only the caller's
profiles, and read, replace, delete, and preference operations deliberately
answer `404 environment_profile_not_found` for both an unknown identifier and a
profile owned by somebody else. There is no operator cross-owner read for a
resource whose variable values may contain credentials.

### Write shape: `ProfileRequest`

`POST` creates a profile and `PUT /{id}` fully replaces its writable contents.
This field-complete create example is the canonical request shape:

```http
POST /api/v1/environment-profiles
Content-Type: application/json
```

<!-- profile-request-example -->
```json
{
  "name": "field-complete profile",
  "variables": [
    { "key": "PROFILE_MODE", "value": "round-trip" }
  ],
  "packages": [
    { "name": "platform-cli", "version": "2.4.1" },
    { "name": "ripgrep", "version": "14.1.1" }
  ],
  "agent": {
    "kernel": "profile-kernel",
    "mode": "conversation",
    "model": "model-for-round-trip",
    "reasoning_effort": "x_high",
    "permission": "unrestricted",
    "credential": "own",
    "size": { "cpu_millis": 1500, "memory_mib": 3072 },
    "record_tool_bodies": true,
    "skills": ["mandatory-skill", "chosen-skill"]
  },
  "nyxid": {
    "services": ["mandatory-service", "calendar"]
  },
  "agent_notes": "Keep the caller's standing instructions byte for byte."
}
```

Top-level fields:

| Field | Required | Default and meaning |
|---|---:|---|
| `name` | yes | Owner-visible name. Whitespace is trimmed; an empty name is refused |
| `variables` | no | `[]`. Each entry requires string `key` and `value`. Values are opaque, stored, and returned so an editor can round-trip them |
| `packages` | no | `[]`, before mandatory packages are merged. Each entry requires `name`; `version` is optional and is omitted from a response when unpinned |
| `agent` | no | `{}`. The nested defaults are listed below |
| `nyxid` | no | `{ "services": [] }`, before mandatory services are merged |
| `agent_notes` | no | `""`. Owner-authored standing instructions, separate from non-editable platform instructions |

`agent` fields:

| Field | Required | Default and meaning |
|---|---:|---|
| `kernel` | no | The deployment's configured kernel on create and full replacement. Replacement omission does not preserve the current explicit selection; echo the value from GET to retain it |
| `mode` | no | `"terminal"`; the other value is `"conversation"` |
| `model` | no | No pinned model. It is omitted from the response when absent |
| `reasoning_effort` | no | `"high"`; accepted values are `minimal`, `low`, `medium`, `high`, and `x_high` |
| `permission` | no | `"guarded"`; the other values are `autonomous` and `unrestricted` |
| `credential` | no | `"delegated"`; `"own"` means the agent uses the owner's model-vendor account. This is a mode, never credential material |
| `size` | no | The deployment's reservation. When present, both positive integer fields `cpu_millis` and `memory_mib` are required; absent size is omitted from the response |
| `record_tool_bodies` | no | `false`. Whether tool arguments and results are written to the durable conversation log. The response always carries this boolean |
| `skills` | no | `[]`, before mandatory skills are merged. Entries are skill identifiers |

Each writable list accepts at most 100 caller-controlled entries. `packages`,
`skills`, and `nyxid.services` may render more than 100 entries after the
deployment adds its mandatory baseline. A replacement accepts either the six
writable fields or the complete `ProfileView` returned by GET, so a client may
literally GET, transform writable content, and PUT that object back. In the full
view, `id` must match the path; `mandatory`, `created_at`, `updated_at`, and
`preferred` are type-checked read-only echoes and never overwrite current
server-owned state. Those five fields remain invalid on create. An exact raw-view
or writable-projection echo remains a no-op after the mandatory baseline grows,
shrinks, or renames entries. Any changed replacement is validated against the
current baseline: one occurrence of each current mandatory identifier is
deployment-owned, while duplicates and retired mandatory identities count toward
the caller's limit.

Every object in this request is closed: unknown fields are refused at the top
level and inside `variables`, `packages`, `agent`, `agent.size`, `nyxid`, and a
replacement view's `mandatory` object.
Create and replace both return `422 invalid_request` rather than silently
discarding a misspelling. For a short identifier-shaped unknown key, the
problem document names it in `field` and gives its object location in `path`
(for example, `"field": "reasoningEffort"` and
`"path": "agent.reasoningEffort"`). The human-readable detail stays fixed and
no submitted scalar value is reflected. Non-identifier and unbounded keys are
also refused but omitted from the diagnostic fields. For example, this is
invalid:

<!-- profile-invalid-request-example -->
```json
{
  "name": "misspelled profile",
  "agent": { "reasoningEffort": "x_high" }
}
```

### Read shape: `ProfileView`

Create, read, replace, list, and preference responses use one profile shape.
Every accepted write field is rendered back. Server-owned fields are added:

| Field | Meaning |
|---|---|
| `id` | Stable profile identifier |
| `name`, `variables`, `packages`, `agent`, `nyxid`, `agent_notes` | The writable contents after mandatory entries have been applied |
| `mandatory.packages` | Package names the deployment puts back on every write |
| `mandatory.skills` | Skill identifiers the deployment puts back on every write |
| `created_at` | Creation time, retained across replacement |
| `updated_at` | Last content-changing replacement time; an exact raw-view or writable-projection echo retains it |
| `preferred` | Client-facing preselection state. A client may start its session form on this profile, but must still send its id in `POST /api/v1/sessions` |

`agent.kernel`, `mode`, `reasoning_effort`, `permission`, `credential`,
`record_tool_bodies`, and `skills` are always present. `agent.model` and
`agent.size` are omitted when no value was selected. `packages[].version` is
likewise omitted for an unpinned package. A boolean such as
`record_tool_bodies` is never made ambiguous by omission.

A profile stored before the deployment added a mandatory entry or began
settling kernels is projected through the current baseline and configured
kernel when read. This does not rewrite the stored record or advance
`updated_at`; it gives legacy and newly written profiles the same writable
projection in the wire shape.

**`mandatory` marks entries, not a special profile.** Omitting a mandatory
package or skill from a replacement does not remove it; the service merges it
back and reports its identifier under `mandatory` so a client can render the
entry as fixed. An owned profile containing mandatory entries can still be
deleted with `204`; deleting the profile does not revoke the deployment's
baseline from other profiles or sessions.

**`201 Created`** returns a complete `ProfileView`. On creation, `created_at`
and `updated_at` are identical because no later content replacement has occurred:

<!-- profile-view-example -->
```json
{
  "id": "envp_example",
  "name": "field-complete profile",
  "variables": [
    { "key": "PROFILE_MODE", "value": "round-trip" }
  ],
  "packages": [
    { "name": "platform-cli", "version": "2.4.1" },
    { "name": "ripgrep", "version": "14.1.1" }
  ],
  "agent": {
    "kernel": "profile-kernel",
    "mode": "conversation",
    "model": "model-for-round-trip",
    "reasoning_effort": "x_high",
    "permission": "unrestricted",
    "credential": "own",
    "size": { "cpu_millis": 1500, "memory_mib": 3072 },
    "record_tool_bodies": true,
    "skills": ["mandatory-skill", "chosen-skill"]
  },
  "nyxid": {
    "services": ["mandatory-service", "calendar"]
  },
  "agent_notes": "Keep the caller's standing instructions byte for byte.",
  "mandatory": {
    "packages": ["platform-cli"],
    "skills": ["mandatory-skill"]
  },
  "created_at": "2026-08-25T09:00:00Z",
  "updated_at": "2026-08-25T09:00:00Z",
  "preferred": false
}
```

### List

```http
GET /api/v1/environment-profiles?limit=50
```

**`200 OK`** is a newest-first array of complete `ProfileView` rows:

<!-- profile-list-response-example -->
```json
[
  {
    "id": "envp_example",
    "name": "field-complete profile",
    "variables": [
      { "key": "PROFILE_MODE", "value": "round-trip" }
    ],
    "packages": [
      { "name": "platform-cli", "version": "2.4.1" },
      { "name": "ripgrep", "version": "14.1.1" }
    ],
    "agent": {
      "kernel": "profile-kernel",
      "mode": "conversation",
      "model": "model-for-round-trip",
      "reasoning_effort": "x_high",
      "permission": "unrestricted",
      "credential": "own",
      "size": { "cpu_millis": 1500, "memory_mib": 3072 },
      "record_tool_bodies": true,
      "skills": ["mandatory-skill", "chosen-skill"]
    },
    "nyxid": {
      "services": ["mandatory-service", "calendar"]
    },
    "agent_notes": "Keep the caller's standing instructions byte for byte.",
    "mandatory": {
      "packages": ["platform-cli"],
      "skills": ["mandatory-skill"]
    },
    "created_at": "2026-08-25T09:00:00Z",
    "updated_at": "2026-08-25T09:00:00Z",
    "preferred": false
  }
]
```

`limit` defaults to `50` and is clamped to `1..=200`. `cursor` is the opaque
value from `Link: <...>; rel="next"`; the final page has no next link. Profiles
do not accept the session-only `status` filter. Unknown parameters, malformed
cursors, or a cursor issued for another collection return `400 invalid_query`.

### Read, replace, and delete

```http
GET /api/v1/environment-profiles/{id}
```

**`200 OK`** returns the complete `ProfileView` shown above. Unknown and foreign
identifiers both return `404 environment_profile_not_found`.

Replacement is a full-document PUT, not a patch:

```http
PUT /api/v1/environment-profiles/{id}
Content-Type: application/json
```

<!-- profile-request-example -->
```json
{
  "name": "replacement profile",
  "agent": {
    "kernel": "profile-kernel",
    "record_tool_bodies": false
  }
}
```

**`200 OK`** returns the replaced `ProfileView`. Omitted writable fields reset
to their documented defaults, after which mandatory entries are merged back.
The profile keeps its `id`, `created_at`, and `preferred` value. A malformed or
unknown field returns `400` or `422`; an unknown or foreign id returns `404`.
An exact complete-`ProfileView` echo (or its writable projection) whose normalized
contents already match performs no store write and retains `updated_at`, so the
returned `ProfileView` stays byte-identical. Read-only fields in a transformed
view do not overwrite current server state. That includes legacy profiles
projected through a baseline or kernel that changed after they were stored.

```http
DELETE /api/v1/environment-profiles/{id}
```

**`204 No Content`** deletes an owned profile, including one that contains
mandatory entries. Unknown and foreign ids return `404
environment_profile_not_found`.

### Skills catalogue

```http
GET /api/v1/environment-profiles/skills
```

**`200 OK`** returns selectable skills and the mandatory package/skill markers
needed before the first profile exists:

<!-- skill-catalog-response-example -->
```json
{
  "skills": [
    {
      "id": "chosen-skill",
      "name": "Repository review",
      "description": "Review a repository without changing it",
      "access": "owner"
    }
  ],
  "mandatory": {
    "packages": ["platform-cli"],
    "skills": ["mandatory-skill"]
  }
}
```

`access` is optional display text supplied by the catalogue. A deployment with
no skill catalogue returns the same shape with an empty `skills` array; profile
writes remain available. If a configured skill catalogue cannot be read, this
route returns a `503 runtime_unavailable` problem document; retrying the picker
request is appropriate.

### Services catalogue

```http
GET /api/v1/environment-profiles/services
```

**`200 OK`** returns services visible to this caller and the identifiers the
deployment mandates:

<!-- service-catalog-response-example -->
```json
{
  "services": [
    {
      "id": "calendar",
      "name": "Calendar",
      "description": "Read and manage calendar events"
    }
  ],
  "mandatory": ["mandatory-service"]
}
```

`description` is optional. The catalogue is queried as the caller because its
answer depends on their grants. With no service catalogue configured, the
route returns `200` with an empty `services` array; profiles still store service
selections and sessions still honour them. That absence is distinct from a
configured catalogue outage: when the catalogue cannot be asked, this route
returns a `503 service_catalogue_unavailable` problem document and the caller
should retry.

### Preferred profile

Preference is client-facing preselection state. It is a collection-level PUT
because selecting one profile also clears the previous winner; it is not a
writable flag on an individual profile. A client may initialize its session
form from the selected profile, but it must send that id explicitly as
`environment_profile` when creating a session. The service never consults this
state for an unqualified create, which always uses deployment defaults.

```http
PUT /api/v1/environment-profiles/preferred
Content-Type: application/json
```

<!-- preferred-request-example -->
```json
{ "profile": "envp_example" }
```

`profile` must identify one profile the caller owns. The selected profile is
validated before the previous preference is changed. Unknown and foreign ids
both return `404 environment_profile_not_found`; a malformed or unknown body
field returns `400` or `422`.

**`200 OK`** is the complete post-write profile collection, newest first, with
exactly the selected row carrying `preferred: true`:

<!-- preferred-response-example -->
```json
[
  {
    "id": "envp_example",
    "name": "field-complete profile",
    "variables": [
      { "key": "PROFILE_MODE", "value": "round-trip" }
    ],
    "packages": [
      { "name": "platform-cli", "version": "2.4.1" },
      { "name": "ripgrep", "version": "14.1.1" }
    ],
    "agent": {
      "kernel": "profile-kernel",
      "mode": "conversation",
      "model": "model-for-round-trip",
      "reasoning_effort": "x_high",
      "permission": "unrestricted",
      "credential": "own",
      "size": { "cpu_millis": 1500, "memory_mib": 3072 },
      "record_tool_bodies": true,
      "skills": ["mandatory-skill", "chosen-skill"]
    },
    "nyxid": {
      "services": ["mandatory-service", "calendar"]
    },
    "agent_notes": "Keep the caller's standing instructions byte for byte.",
    "mandatory": {
      "packages": ["platform-cli"],
      "skills": ["mandatory-skill"]
    },
    "created_at": "2026-08-25T09:00:00Z",
    "updated_at": "2026-08-25T09:00:00Z",
    "preferred": true
  }
]
```

Clearing is expressed with an explicit `null` value:

<!-- preferred-request-example -->
```json
{ "profile": null }
```

It returns the same complete collection with every row carrying
`preferred: false`. The `/v1` response is intentionally complete for clients
that replace their loaded collection with it; it is assembled through bounded
owner-list pages even though the final JSON array has no profile-count limit.

---

## 3. Sessions

### Create

```http
POST /api/v1/sessions
Content-Type: application/json

{
  "cols": 120,
  "rows": 32,
  "environment_profile": "envp_7dec1b0ca73c46b88121b259577bbe96",
  "quoted_rate_card": "rate_9f2c4d71"
}
```

| Field                 | Required | Notes                                                                                                                                         |
| --------------------- | -------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `cols`                | no       | Initial terminal width. Sent with `rows` or not at all                                                                                          |
| `rows`                | no       | Initial terminal height                                                                                                                         |
| `environment_profile` | no       | Id from [Environment Profiles](#2-environment-profiles). **Never carries secret values** — it selects a profile provisioned out of band. Omitting it is a choice too: the session starts the way this deployment says an unqualified one should |
| `quoted_rate_card`    | no       | The `rate_card` from `GET /api/v1/pricing/quote`. If another card is now in force, creation is refused with `409 price_changed` before any session exists. Omit it for backward compatibility when no quote was taken |

**These four are the whole body.** An earlier version of this section
documented `llm_source` as required, along with `environment`, `workspace`,
`concurrent_input` and `metadata`, and an `Idempotency-Key` header on every
side-effecting `POST`. None of them has ever existed in the service. They were
accepted and discarded in silence, so a caller who built to that contract got a
working session and no indication that most of what they sent went nowhere.
They are now refused by name, which is why the list above is exact rather than
aspirational.

The profile is the whole description of the session — which agent, which model,
how freely it acts, whose access it spends, what it carries, and how much CPU
and memory it reserves — so nothing else about the agent is sent here. The size
is the profile's: omitting `agent.size` takes the deployment's reservation, and
the answer is frozen for the life of the session, including across a rotation.

**A caller does not choose a kernel here.** Which one a session runs on comes
from the [environment profile](#2-environment-profiles) it names, or from what
the deployment configured when it names none. There is no session-create field
for it, and that is deliberate: the reusable profile is the one place that
states the complete agent configuration.

**A field this endpoint does not know is refused, not ignored.** Sending
`"kernel"` is the obvious thing to try, because `GET /api/v1/kernels`
enumerates them — so it answers `422` with `invalid_request`. Rejection details
are fixed by error class rather than copied from Serde because other request
bodies can carry credentials. It used to be discarded, and the session came
back `201` on whichever kernel the profile actually resolved to. To choose a
kernel, create or replace an [environment profile](#2-environment-profiles)
whose `agent.kernel` is the one you want, then name its `id` here.

A body that cannot be read keeps the status that says why: `415` when the `Content-Type` is not `application/json`, `400` when the JSON is malformed, `422` when it parsed and could not be honoured. All three carry the same problem document as every other failure.

The profile is **frozen at creation**, as is the kernel it resolves to. Changing either means a new session.

**`201 Created`** — the shape below was read off a live deployment rather than
composed here, because the previous example carried an `llm_source` field the
service does not send and spelled `recovery_state` as an object it does not use.

```json
{
  "id": "sesn_1c83fa5f89e84f2da092398e669a12f6",
  "status": "provisioning",
  "recovery_state": "normal",
  "kernel": "codex",
  "agent": {
    "permission": "guarded",
    "reasoning_effort": "high",
    "credential": "delegated"
  },
  "cols": 120,
  "rows": 32,
  "last_seq": 0,
  "created_at": "2026-08-19T11:38:12Z",
  "conforms_to": [
    "https://spec.chrono-ai.fun/cma/1.0/conf/core",
    "https://spec.chrono-ai.fun/cma/1.0/conf/conversation"
  ],
  "reach": []
}
```

`conforms_to` is what this session was granted, and is how a client learns what
it may ask for without trying. `runtime_version`, `rotations` and `rotated_at`
appear once a session has been moved, and are absent before that because a
session that has never been rotated has nothing to explain.

A priced session also carries the price settled when it was created:

```json
{
  "pricing": {
    "rate_card": "rate_9f2c4d71",
    "currency": "USD",
    "micros_per_hour": "45000",
    "display": "$0.045",
    "quoted_at": "2026-08-23T05:00:00Z"
  }
}
```

This is part of the session record, not a fresh lookup. The card may have been
retired since creation and the session still carries the rate it accepted.
`pricing` is omitted for a session created before pricing was recorded or on a
deployment with no configured price. Omission means **not priced**; it never
means zero or free.

### Live-session allowance

```http
GET /api/v1/sessions/allowance
```

```json
{ "limit": 3, "live": 2, "exempt": false }
```

This is the authenticated caller's own allowance, including for an operator
who may read other owners' sessions. `limit` is absent when this deployment has
set no limit. `exempt` is true only for a caller explicitly allowed to exceed
the limit; it is not inferred from an absent limit.

`live` counts records whose desired state wants a runtime: `provisioning`,
`ready`, `running`, and `rotating`. It is read from the session record, never
from provider inventory. An `idle` (paused) session consumes no slot, nor does
a terminated one. Reviving a paused session is checked as a new ask, which is
what prevents pause and revive from walking around the limit.

**This is advisory.** It exists so a client can offer only what will be
permitted, rather than offering everything and letting people discover
refusals by hitting them. The core checks again on the create or revive request
that uses it, so a client that ignored this answer, or fabricated one, would
gain nothing. Hiding a control is a courtesy; it is not access control.

Lowering a limit is **not retroactive**. Existing work is never stopped, and a
control-plane restart, reconciliation sweep, or runtime rotation brings every
recorded live session back regardless of the new limit. An owner already over
the limit cannot create or revive until enough sessions have been paused or
ended to put them below it. Repeating revive on a session that already wants a
runtime remains a no-op and does not consume a second slot.

Enforcement serialises asks by one subject inside each control-plane replica.
Across replicas it is best-effort: concurrent asks can overshoot the configured
limit by a bounded amount. This is a policy limit, not a security boundary; a
hard distributed limit requires a conditional write in the session store.

### Read, list, stop, revive, delete

```http
GET    /api/v1/sessions/{id}
GET    /api/v1/sessions?status=running&limit=50
POST   /api/v1/sessions/{id}/stop
POST   /api/v1/sessions/{id}/revive
GET    /api/v1/sessions/{id}/transcript
DELETE /api/v1/sessions/{id}
```

`GET /api/v1/sessions` returns `200 []` only when the session store was read
successfully and the caller genuinely has no visible sessions. If the store
cannot be read, it returns a `503 runtime_unavailable` problem document; an
unavailable list is never represented as an empty one.

The session list is newest first and remains a JSON array. It accepts exactly
these query parameters:

| Parameter | Meaning |
|---|---|
| `status` | Exact lifecycle value to retain, such as `running`; omit it for every status |
| `limit` | Page size, default `50`, clamped to `1..=200` (`0` becomes `1`) |
| `cursor` | Opaque continuation copied from the previous response's next link |

Rows are stably ordered by `created_at` descending and then session id
descending. When another page exists the response carries
`Link: </api/v1/sessions?...&cursor=...>; rel="next"`; the final page has no
next link. Follow that link verbatim rather than decoding or constructing its
cursor. Cursors are versioned and bound to the exact collection that issued
them: owner, operator, and environment-profile cursors are not interchangeable.
They also carry a canonical RFC 3339 store position. A misspelled or unknown
parameter; malformed cursor encoding or payload; an unsupported cursor version;
a noncanonical or nonsemantic cursor timestamp; an empty cursor identifier; or a
cursor reused on another collection is refused with `400
application/problem+json` and `code: "invalid_query"` rather than being silently
ignored.

`stop` releases the runtime and keeps the session: it goes `idle`, stays listed,
stays readable, and nothing is being paid for. `revive` gives it a runtime
again under the same id, with the same history and the same options — reviving
never mints a new session. When the owner is at or over their live-session
limit, revive returns the same typed refusal as create and the session stays
paused.

The session returned by `revive` uses the same projection as create, list, and
read. Its creator, granted reach, and recorded installed skills do not disappear
because the route changed. Directory or catalogue outages cost human-readable
names, not identifiers or the response itself. A newly created provisioning
session can omit `agent.skills` until installation has actually been recorded.

`DELETE` is terminal and irreversible. It is the only one of the three that loses anything, and it is not how you pause.

`transcript` returns everything the session's terminal has shown, as the byte stream a terminal renders — `application/octet-stream`, not JSON, because escaping an escape-sequence stream into a string is a decision the server has no business making. It works whether or not a runtime exists, which is how a stopped session stays readable.

### Environment profile list

```http
GET /api/v1/environment-profiles?limit=50&cursor=<opaque>
```

The owned-profile list uses the same stable newest-first array and pagination
contract: `limit` defaults to `50` and is clamped to `1..=200`, `cursor` is
opaque, and `Link: rel="next"` appears only when another page exists. Profiles
do not accept the session-only `status` filter. Unknown parameters and malformed
cursors return the same `400 invalid_query` problem document.

### Preferred environment profile

```http
PUT /api/v1/environment-profiles/preferred
Content-Type: application/json

{ "profile": "envp_01K3" }
```

`profile` names one profile the caller owns. It is client-facing preselection
state: a client starts its session form on that profile and sends the selected
id explicitly in `POST /api/v1/sessions`. `null` clears the preselection. The
service never substitutes a preferred id into an unqualified create request;
omitting `environment_profile` always uses the deployment's defaults. The
selected profile is validated before the existing preference is changed.

The `200 OK` body is the complete post-write profile collection, newest first,
with exactly the selected row carrying `preferred: true`. Clearing the choice
returns the same complete collection with every row carrying `preferred: false`.
This is the response released under `/v1`; existing clients replace their
loaded collection with it, so narrowing it would make unchanged profiles
disappear.

The mutation itself remains bounded: it validates the selected profile, reads
the previous winner, and asks the store to replace the preference without
collecting the owner's profiles. After that succeeds, the compatibility
response is assembled through the same owner-authorized list service as the
GET route, walking newest-first store pages of at most 200 rows. The response
is intentionally complete, but no individual persistence read is unbounded and
there is no profile-count quota.

### Changing which plane a session is driven on

```http
POST /api/v1/sessions/{id}/plane
Content-Type: application/json

{ "plane": "conversation" }
```

`terminal` or `conversation`. Naming the plane the session is already on changes nothing and is not an error.

**It is a rotation.** The session's conversation is captured, its sandbox is released, a replacement is started under the other front-end, and the agent resumes from the capture. The session keeps its identifier, its history, its configuration and its place in its owner's list; `rotations` and `rotated_at` move exactly as they do for a runtime upgrade, and `rotated_at` is the moment the conversation was taken — anything the agent did after it did not travel.

**`200 OK`** returns the session mid-handover, `status: "rotating"`. That is the window in which a client must refuse input rather than accept and lose it; the session becomes usable again on its own, and the event stream says when.

The response still carries the session's creator, granted `reach`, and recorded
installed skills, with catalogue names whenever available. A handover changes
its plane; it does not turn a populated session into a partial representation.

The granted set is re-derived and re-reported: a session moved to `conversation` stops conforming to `…/conf/terminal` and starts conforming to `…/conf/conversation`. A client that cached the old set will be refused by name, which is the point of refusing by name.

**Only the owner.** An operator capability does not stand in here: ending somebody's session is something an operator has to be able to do, and deciding what they are looking at is not.

Every refusal comes before anything is torn down, and a refused session keeps running exactly as it was:

| Situation | Status | `code` |
|---|---|---|
| The kernel does not serve that plane | `422` | `capability_unsupported`, naming `conversation` or `terminal` |
| The kernel cannot carry its conversation across | `422` | `capability_unsupported`, naming `snapshot` |
| The session is already being moved, or has no runtime | `409` | `invalid_state` |
| The session is somebody else's | `404` | `session_not_found` |

### What a session may reach

A read carries `reach`: the services this session's credential may call on its owner's behalf.

```json
"reach": [
  { "id": "api-github", "name": "GitHub", "description": "Repositories and pull requests", "mandated": false },
  { "id": "nyxid", "name": "NyxID", "mandated": true }
]
```

This is the other half of a fact whose first half is `capabilities`. Capabilities are what a session can **do**; `reach` is what it can **touch**. A client showing one and not the other is describing half a session.

The whole path — catalogue, profile, grant, credential, sandbox — is documented
in [Services](./Services.md).

**It is the session's answer, not its profile's.** A session's reach is decided when it is provisioned and recorded on the session. A profile can be edited afterwards — widened, narrowed, or deleted — and the session keeps the scope it was provisioned with, so reading the profile to learn a running session's reach gives the wrong answer precisely when somebody has changed one. It survives a rotation for the same reason: a replaced runtime is the same session.

`mandated` marks what this deployment requires in every session, so a client can draw those entries as fixed rather than guessing which cannot be removed.

`name` and `description` come from the caller's own service catalogue and are **absent when it could not be asked**. A catalogue that is down costs the names and not the answer: the identifiers alone are still true, and reporting an error here would render a transient outage as a session that reaches nothing. See `docs/Services.md`.

**Never a credential.** `reach` carries identifiers and display names. What a session may reach is a fact about its configuration; what it holds is not, and no token, no expiry and nothing that could be used to fingerprint one appears here.

---

## 4. Input, interrupt, steer

### One route, two planes

`input` means different things to the two kinds of session, and the session's granted capabilities decide which. A caller never chooses: read `capabilities` on the session object, or just send — the projection is the same operation in the core either way.

**Conversation session** — the text opens a prompt turn.

```http
POST /api/v1/sessions/{id}/input
{ "text": "Refactor the auth module to use the new token type." }
```

**Terminal session** — the text is typed at the terminal, with a newline unless you say otherwise.

```http
POST /api/v1/sessions/{id}/input
{ "text": "ls -la", "newline": true }
```

`newline` defaults to `true`: a client sending a line means to submit it. Send `false` to compose a command in pieces.

For anything a JSON string cannot carry — control characters, escape sequences, and the keypresses that answer a kernel's own on-screen approval prompt — send raw bytes instead:

```http
POST /api/v1/sessions/{id}/input
{ "bytes_b64": "GwtC" }
```

`bytes_b64` is refused on a conversation session: a conversation takes words, and decoding bytes into a prompt would send something nobody typed. Sending both fields on a terminal session uses the bytes — a caller who supplied them meant something the text could not express.

**On the terminal plane, input is how approvals are answered and how a turn is steered.** That kernel asks on its own screen, and the answer is a keypress. There is no separate operation for it, and §6 says so where a client would look.

### Interrupt

```http
POST /api/v1/sessions/{id}/interrupt
```

Stops what the session is doing without ending it. On a conversation this cancels the turn in flight; on a terminal it injects the in-band `0x03` control byte (Ctrl-C), not an OS signal.

Interrupting requires the `interrupt` capability on a conversation session. Not every kernel has one — a wire can offer a prompt and no way to withdraw it — and a session whose kernel cannot is refused by name rather than left waiting for something that will not happen.

### Steer

```http
POST /api/v1/sessions/{id}/steer
{ "text": "Stop — only touch files under src/auth/." }
```

Redirects a turn already in progress, which is not the same as starting a new one. Projected only where the kernel advertises `steer`; elsewhere it returns the typed capability error. Subsets are declared, not discovered.

Sending input while a turn is in flight resolves according to the session's `concurrent_input` policy; on `reject` it returns `409` with `problem.type` ending `/turn-in-progress`.

### While a terminal is attached

A live PTY attachment holds the terminal exclusively, and that changes what these routes do:

| | while attached | reason |
|---|---|---|
| `input` | `409` `/already-attached` | keystrokes injected into a live attachment land inside an escape sequence that client is drawing; afterwards neither side can tell which characters were whose |
| `resize` | `409` `/already-attached` | the attached client declared the geometry it is drawing for, and resizing reflows a screen it is halfway through painting |
| `interrupt` | **allowed** | it injects only the bounded, named `INT` control byte (`0x03`, Ctrl-C), rather than free-form input, and stopping a runaway command is exactly what an owner may need to do from elsewhere |

The rule is about *this* route, not about the terminal. The client holding the attachment drives it through its own connection and is unaffected: refusing writes at the operation itself would refuse that client its own keystrokes.

With no attachment live, all three operate the terminal directly.

---

## 5. Events

### Replay

```http
GET /api/v1/sessions/{id}/events?starting_after=1042&limit=200
```

Returns durable events in `seq` order, oldest first. `seq` is **monotonic per session**, so a cursor is one integer. Use `Link: rel="next"` to page.

### Stream

```http
GET /api/v1/sessions/{id}/events/stream?starting_after=1042
Accept: text/event-stream
```

Server-sent events. Each carries `id` (the `seq`) and `event` (the type).

```
event: turn.started
id: 1043
data: {"session_id":"sesn_01JD8Z3K7","seq":1043,"type":"turn.started"}

event: approval.requested
id: 1044
data: {"session_id":"sesn_01JD8Z3K7","seq":1044,"type":"approval.requested",
       "request_id":"apr_7Q2","action":"command_execution","detail":{"command":"rm -rf build/"}}
```

**Reconnect correctly:** open the stream first, then `GET /events` for history, then deduplicate by `seq`. The stream does not replay what it missed while closed; `starting_after` does.

Delta events (`output.delta`, `thinking.delta`) are **stream-only and never persisted**. They exist for liveness. Anything durable arrives as a semantic event.

A runtime the service established as lost arrives in an unnamed SSE frame with the core kind nested under `event`, exactly as the implementation sends it:

```text
id: 1045
data: {"seq":1045,"session_id":"sesn_01JD8Z3K7","event":{"type":"runtime_lost","runtime_id":"runtime_01K35QJ0","cause":"out_of_memory"}}
```

`runtime_lost` carries `runtime_id`, the opaque identifier of the runtime that is gone, and `cause` where the service could establish one. The cause is `out_of_memory`, `ended`, `failed`, `displaced`, or `unrecognised`; when nothing supplied a reason, `cause` is omitted rather than invented.

Memory pressure is recorded as two edge-triggered events. Raw resource
readings are never written to the event log: each reading replaces the prior
live value, while an episode is something that happened once and must survive
a reconnect.

```json
{"type":"memory_pressure_opened","used_bytes":912680550,"limit_bytes":1073741824,"occupancy_permille":850}
{"type":"memory_pressure_closed","peak_bytes":1041529569,"limit_bytes":1073741824,"peak_permille":970,"duration_seconds":181}
```

`memory_pressure_opened` appears only after occupancy has stayed at or above
the configured entry threshold for the configured consecutive-reading dwell;
a brief build spike therefore produces no event. `memory_pressure_closed`
appears after occupancy has stayed below the exit threshold and the settle
floor has elapsed. It carries the episode's peak and a duration measured by a
monotonic clock, not by subtracting the event timestamps.

For client folding, an open episode ends as soon as the session leaves an
interactive status, even if no close was written (for example, if the control
plane stopped during the episode). A second `memory_pressure_opened` without a
close replaces the first; pressure is state, not a stack. A reading with no
memory figure or no limit says nothing about pressure and neither opens nor
closes an episode.

---

## 6. Approvals

```http
POST /api/v1/sessions/{id}/approvals/{request_id}
{ "decision": "allow" }
```

`decision` is `allow`, `deny`, or `abort`. Refusing one action and abandoning the turn are different intentions: `deny` refuses this and the turn carries on without it, `abort` refuses it and stops the turn. A client that meant the second and sent the first watches the agent try something else instead of stopping.

**Conversation sessions only.** On a terminal session this returns the typed capability error naming `conversation`: that kernel asks on its own screen and its answer is a keypress, so the operation to use is `POST /input` with the bytes the prompt is waiting for (§4). Inventing an approval route there would be a second way to say something the session cannot hear.

While an approval is pending the session is `idle` with `stop_reason: requires_action`, and the pending request ids are listed on the session object. That state survives a client disconnect and a control plane restart — a request waiting when you reconnect is still answerable. Answering one nothing is waiting for is refused rather than forwarded.

---

## 7. Terminal

```http
GET  /api/v1/sessions/{id}/screen          → current rendered screen
POST /api/v1/sessions/{id}/resize          { "cols": 120, "rows": 40 }
POST /api/v1/sessions/{id}/terminal-ticket { "takeover": false }
```

REST exposes these for clients that need terminal state without holding a stream. The live byte stream itself is the [PTY interface](./API_PTY.md).

`resize` with a zero dimension is rejected with `422`.

### Attach ticket

```json
{ "ticket": "…", "expires_at": "2026-08-17T14:37:14Z" }
```

The browser WebSocket API cannot set a header on an upgrade, so a bearer-authenticated frontend exchanges its credential here — over a route that *can* carry one — for a single-use, ~30-second, session- and subject-bound ticket, and presents that as `?ticket=…` on the upgrade. Same-origin cookie clients need none of this and attach unchanged.

`{"takeover": true}` supersedes whoever currently holds the attachment; the superseded client is sent a close frame naming the reason. Without it, attaching over a live attachment is a typed conflict.

A session with no terminal capability is refused here with `capability_unsupported`, not issued a ticket for something it cannot do. The full contract, including what makes a ticket in a URL acceptable and what is never recorded, is in [API_PTY.md §2.1](./API_PTY.md).

**Both are owner-only, and no operator capability reaches them.** A screen carries whatever the agent has printed, and a session started from an environment bundle has that bundle's values in its process environment — so what is on a screen may be its owner's secrets. This is the same rule `transcript` follows, for the same content by another route.

---

## 8. Pricing and usage

### Quote a new session

```http
GET /api/v1/pricing/quote?environment_profile=envp_7dec1b0ca73c46b88121b259577bbe96
```

The route is authenticated. `environment_profile` is optional; omission asks
for the deployment's unqualified session, exactly as omission on create does.
The service resolves the profile's actual authored size and computes the quote
with integer micro-USD arithmetic:

```json
{
  "rate_card": "rate_9f2c4d71",
  "currency": "USD",
  "cpu_millis": 1000,
  "memory_mib": 1024,
  "micros_per_hour": "45000",
  "display": "$0.045",
  "per": "hour",
  "quoted_at": "2026-08-23T05:00:00Z"
}
```

`micros_per_hour` is a decimal string so its complete integer survives clients
whose JSON numbers are binary floating point. `display` is the lossless string
the service will reconcile against; clients render it verbatim and perform no
rounding. The only currency is USD. Memory is priced in GiB, with 1,024 MiB per
GiB.

There is deliberately no raw CPU/memory query. A quote accepts an environment
profile because it must price the size the service will actually provision,
not a hypothetical size a client assembled separately. If a price is
configured but the selected session has no authored size, the route returns
`422 invalid_request`, naming the missing administrator-declared assumed
session size. The service does not parse opaque deployment quantity strings or
invent a default.

Where no price list is configured, the successful response is exactly:

```json
{ "priced": false }
```

That is not a zero quote and is never rendered as `$0.00`. The deployment is
working and has stated no price.

This route quotes a **new** session. There is no route to re-quote a session
that already exists, deliberately: its `pricing` field is the rate pinned at
creation, and looking up the current card would give the wrong answer as soon
as prices change.

Send the returned `rate_card` as `quoted_rate_card` on `POST /api/v1/sessions`.
The amount pinned on the new session is computed from that same sealed card and
resolved size. If another card came into force between the two requests,
creation returns `409 price_changed` and writes no session; read a fresh quote
and ask the owner again.

### Runtime usage

```http
GET /api/v1/sessions/{id}/usage
```

```json
{ "input_tokens": 184203, "output_tokens": 9877, "context_used": 41204, "context_window": 200000 }
```

Requires the `usage` capability. Not every kernel reports token counts, and one that does not returns the typed capability error rather than a plausible zero.

`context_window` is the ceiling the runtime stated for `context_used`. The key is absent where the runtime states none and is never zero; no interface estimates one.

`input_tokens` and `output_tokens` are the **highest** totals this session's runtimes have reported, not the latest and not their sum. A session carried onto a replacement runtime is counted afresh by that runtime: taking the latest would show usage falling, and adding them would count the history the new runtime resumed from twice. `context_used` is how full the context is *now*, so it does fall when the agent summarises the conversation to make room. Each such fall is recorded as a durable `context_compacted` event carrying the previous reading, the new reading and the stated ceiling where one is known.

### Live sandbox resources

```http
GET /api/v1/sessions/{id}/resources
```

The latest reading is a point read beside `/usage`. It carries all fourteen
figures the core holds, in REST's snake-case vocabulary:

```json
{
  "reading": {
    "cpu_millis_used": 340,
    "cpu_millis_limit": 1000,
    "memory_bytes_used": 892211200,
    "memory_bytes_limit": 1073741824,
    "memory_bytes_peak": 930086912,
    "stall_permille": 12,
    "ceiling_hits": 0,
    "allocation_failures": 0,
    "kills": 0,
    "received_bytes": 18442,
    "sent_bytes": 9120,
    "received_bytes_per_second": 420,
    "sent_bytes_per_second": 110,
    "interval_ms": 2000
  },
  "observed_at": "2026-08-22T09:14:06Z",
  "reason": null
}
```

Every field is present. An unread level, an absent limit, and a rate without a
predecessor are each `null`, never omitted and never zero. `observed_at` is the
UTC RFC 3339 moment the reading reached the core.

The route has five distinguishable answers:

| Condition | Answer |
|---|---|
| The session was not granted `resources` | **422** `capability-unsupported`, with `"capability": "resources"`; never a 404 |
| Granted, but no reading has arrived | **200**, `{"reading":null,"observed_at":null,"reason":"no_reading_yet"}` |
| Granted, but this runtime cannot report | **200**, `{"reading":null,"observed_at":null,"reason":"runtime_does_not_report"}` |
| A reading arrived without a ceiling | **200**, the complete reading with the limit field `null` and `reason:null` |
| The caller cannot see the session | **404** `session-not-found`, indistinguishable from a missing id |

A granted session declares
`https://spec.chrono-ai.fun/cma/1.0/conf/resources` in its own
`conforms_to` list. An older record whose resource grant was never written, or
a session explicitly refused the grant, does not.

### Reserved-capacity spend

```http
GET /api/v1/sessions/{id}/spend
```

This owner-only route reads what the session has accrued at the rate pinned
when it was created. It is separate from `/usage`: this is the deployment's
charge for reserved CPU and memory, while token usage and any model-vendor cost
describe a different service and may be billed to a different party.

A priced session returns:

```json
{
  "priced": true,
  "rate_card": "rate_9f2c4d71",
  "currency": "USD",
  "micros_per_hour": "45000",
  "accrued_micros": "22500",
  "display": "$0.0225",
  "billable_seconds": 1800,
  "accruing": true,
  "unbounded_intervals": 0
}
```

`micros_per_hour` and `accrued_micros` are decimal strings in micro-USD;
`display` is rendered by the service from the same pinned integer and is what a
client shows verbatim. The ledger has one-second resolution. It combines all
billable seconds before dividing by 3,600, so the one final floor rounds toward
the owner instead of losing a fraction independently at every transition.

The charge is for the session's recorded reservation, not sampled CPU or
memory utilisation. Requests and limits are the same at the provider boundary,
but the provider does not echo the capacity it applied, so this service cannot
claim that measured or provider-confirmed capacity is being billed. A rotation
is one continuous session interval and overlapping runtimes cannot count a
second twice.

Accrual begins when the session first becomes `ready`. It remains open while
the session is `ready`, `running`, `rotating`, or waiting for its owner to
resolve an approval. Provisioning time and time after an owner stops or
terminates the session are not charged. A sandbox that has died continues to
accrue while the durable history still says the session holds capacity; the
ledger stops only when the loss produces a durable closing transition. It does
not infer a bill from transient provider observations.

An interval with an absent, malformed, pre-creation, or future bound is skipped
rather than guessed. `unbounded_intervals` counts those omissions. A non-zero
value means the displayed figure covers less than the session's whole durable
life and must not be presented as complete.

A session created before pricing was recorded, or while no card was configured,
returns exactly the non-monetary shape below. It is not a zero charge:

```json
{
  "priced": false,
  "accruing": false,
  "unbounded_intervals": 0
}
```

Price quotes, pinned session rates, and accrued amounts use micro-USD; `"412"`
means $0.000412, never $4.12. Monetary values on the quote and spend routes are
decimal strings, never binary floating-point numbers.

---

## 9. Capability declaration

```http
GET /api/v1/conformance
```

```json
{
  "conformsTo": [
  "https://spec.chrono-ai.fun/cma/1.0/conf/rest-core",
  "https://spec.chrono-ai.fun/cma/1.0/conf/events-replay",
  "https://spec.chrono-ai.fun/cma/1.0/conf/approvals",
  "https://spec.chrono-ai.fun/cma/1.0/conf/steering",
  "https://spec.chrono-ai.fun/cma/1.0/conf/interrupt",
  "https://spec.chrono-ai.fun/cma/1.0/conf/terminal",
  "https://spec.chrono-ai.fun/cma/1.0/conf/usage"
  ],
  "version": "v0.7.0",
  "build_sha": "8d74d0e55c39d9170d35f92b7818a5eb923bd201"
}
```

`version` is the release version baked into the API image, or `development` for
a local build that supplied no release version. `build_sha` is the full source
commit baked into the API image, or `development` for a local build that
supplied no commit. Clients use these fields to establish which additive `/v1`
contract a deployment is running; neither field is an authorization or
capability grant.

Representations carry `Link: <…/conf/rest-core>; rel="profile"`, and the service advertises `Link: </openapi.json>; rel="service-desc"`.

### What this deployment can run

```http
GET /api/v1/kernels
```

```json
{ "kernels": [
  { "id": "codex", "modes": ["terminal", "conversation"] },
  { "id": "claude-code", "modes": ["conversation"] }
] }
```

Which runtimes this deployment can back a session with, and which planes each one can be asked to serve. A caller still does not choose a kernel on a session — an environment profile names it — so this is what a client offers while somebody is *authoring* a profile.

Kernel discovery requires authentication because it reports deployment-specific facts. The static `GET /api/v1/conformance` document remains public.

`modes` is derived from what the adapter advertises, through the same code that decides a session's granted set. A pairing listed here is one the deployment will accept, and a pairing absent from it is refused with `capability_unsupported` naming the plane. That is the point of publishing it: the refusal belongs where the choice is being made, not after a session has been asked for.

A runtime that serves neither plane is listed with an empty `modes`. It is still a runtime this deployment knows about, and reporting it as absent would be a different claim.

The machine-readable contract is anonymous `GET /openapi.json` (OpenAPI 3.1).
It is generated from the annotated handlers through the same registrations
that build the router, so adding a documented operation also adds its route.
The document includes the RFC 9457 problem schema used by every refusal.

---

## 10. Errors

```http
HTTP/1.1 409 Conflict
Content-Type: application/problem+json
X-Request-Id: req_01JD8Z4M2

{
  "type": "https://spec.chrono-ai.fun/cma/1.0/problems/turn-in-progress",
  "title": "A turn is already in progress",
  "status": 409,
  "detail": "Session sesn_01JD8Z3K7 has an active turn and its concurrent_input policy is 'reject'.",
  "code": "turn_in_progress"
}
```

| Status | When |
|---|---|
| `400` | Malformed JSON (`code: "malformed_json"`) or query parameters that do not match the route (`code: "invalid_query"`) |
| `401` | Missing or invalid NyxID credential (`code: "not_authenticated"`) |
| `403` | Authenticated, but the session belongs to another subject |
| `404` | No matching REST path (`code: "not_found"`), or no such session/resource visible to this subject |
| `405` | The REST path exists but does not accept this method (`code: "method_not_allowed"`) |
| `409` | Conflicts with current state — turn in progress, session terminated, live-session limit reached, or the quoted rate card is no longer in force |
| `413` | The request body exceeds the service's accepted limit (`code: "request_too_large"`) |
| `415` | A JSON route was sent another media type (`code: "unsupported_media_type"`) |
| `422` | Well-formed but semantically invalid — zero terminal dimension, zero session-size figure, or a stated size outside this deployment's bounds |
| `429` | Rate limited; observe `Retry-After` |
| `501` | The operation exists in the core but this interface does not project it |
| `503` | Control plane recovering or failing over; retry |

Every error response includes a non-empty `code` and uses
`Content-Type: application/problem+json`. Clients tell failures apart by that
code and by status, never by prose. `501` responses name the missing capability
in `detail`, so a client learns what is unavailable rather than guessing.

### The three service refusals

Starting a session against a service can fail for three different reasons, and a client that cannot tell them apart retries the one that will never succeed and gives up on the one that would have worked.

| `code` | Status | What it means | What to do |
|---|---|---|---|
| `invalid_session_size` | `422` | A session size was sent with a zero CPU or memory figure. | Send both figures non-zero. **Retrying unchanged will not help.** |
| `service_not_granted` | `422` | The owner has not granted this service. `detail` names it. | Grant it in NyxID. **Retrying will not help.** |
| `service_catalogue_unavailable` | `503` | The catalogue could not be asked. The owner's services are fine. | **Retry.** Nothing is wrong with the session or the profile. |
| — | — | This deployment has no catalogue at all. | Nothing. It is not an error: profiles still store selections and sessions still honour them, there is simply nothing to choose from. |

Told apart by `code` and by status, never by prose — a client matching on a sentence breaks when somebody rewords one.

`service_catalogue_unavailable` is deliberately not `runtime_unavailable`: a caller told the runtime is unavailable goes and looks at the sandbox provider, which has nothing to do with it.

**The refusal is at provision, not at profile write.** A grant can be revoked between saving a profile and starting a session from it, so the write-time answer is friendlier and the provision-time answer is the true one. A profile naming a service the catalogue does not currently grant is stored, and logged as a warning; the session started from it is what gets refused.

**A catalogue that cannot be asked refuses nothing.** What a session may reach is recorded on the profile; the catalogue exists to help choose, not to authorise. Blocking session creation on NyxID being reachable would take the service down with it, and would do so for selections settled days ago.

See `docs/Services.md` for the whole path.

### Price-change refusal

| `code` | Status | What it means | What to do |
|---|---|---|---|
| `price_changed` | `409` | The accepted rate card is no longer the card used for new work. | Read `/api/v1/pricing/quote` again and show the new `display` before retrying. |

```json
{
  "type": "https://spec.chrono-ai.fun/cma/1.0/problems/price-changed",
  "title": "price changed",
  "status": 409,
  "detail": "the price changed: quoted against rate card rate_old, now rate_new",
  "code": "price_changed",
  "quoted_rate_card": "rate_old",
  "rate_card": "rate_new"
}
```

Both card ids are top-level fields so a client never parses the English
`detail`. The refusal keys only on the rate-card id, never on the platform
settings revision: editing a briefing or another unrelated setting is not a
price change. It is a conflict rather than `429`; time does not clear it and no
honest `Retry-After` exists. Creation is refused before an identifier is minted
or a record is written.

### Size-bound refusal

`size_out_of_bounds` is a `422` response with problem type `size-out-of-bounds`. The request is well formed, but this deployment will not provision the size it states. Retrying the same request unchanged will not help.

The problem document carries the refusal as top-level fields so a client never has to parse `detail`:

| Field | Meaning |
|---|---|
| `dimension` | `cpu_millis` or `memory_mib`; the name includes the unit used by the three figures |
| `requested` | The figure the profile asked for |
| `minimum` | The inclusive lower bound currently configured for that dimension |
| `maximum` | The inclusive upper bound currently configured for that dimension |

The refusal happens when a session is created, before an identifier or record exists, not when its environment profile is saved. Bounds can change after a profile was authored, so an out-of-bounds profile remains writable and is logged as a warning; starting a session from it is the operation refused.

These bounds apply only to a size the profile explicitly states. A profile with no `agent.size`, and a creation naming no profile, use this deployment's own sandbox reservation without being measured against the bounds. That reservation may lie outside the range an administrator has configured: the range governs what a caller may ask for, not the unnamed deployment default.

### Deployment-capacity refusal

`deployment_at_capacity` is a `503` response with problem type
`deployment-at-capacity`. It is not a limit on this caller: the deployment's
declared shared room is smaller than this request, and the caller may own none
of the sessions occupying it. A smaller session may fit, or stopping a session
may release enough room.

```json
{
  "type": "https://spec.chrono-ai.fun/cma/1.0/problems/deployment-at-capacity",
  "title": "deployment at capacity",
  "status": 503,
  "detail": "no room for a session of 2000m CPU and 4096MiB: 750m CPU and 1536MiB free",
  "code": "deployment_at_capacity",
  "requested_cpu_millis": 2000,
  "requested_memory_mib": 4096,
  "free_cpu_millis": 750,
  "free_memory_mib": 1536
}
```

The four top-level integer fields let a client state what was requested and
what is free without parsing `detail`. Retrying may succeed after some session
stops, but no `Retry-After` is offered: elapsed time does not release a
reservation. Creation is refused before an identifier is minted or a record is
written.

The capacity is declared in platform settings, not observed from the shared
sandbox provider. Passing this preflight check therefore cannot prove a later
provider create will succeed. The adapter contract can classify a provider's
exact capacity response and avoid retrying it, but the production probe on
2026-08-23 found that the current hosted provider exposes only a generic
pending state and a create timeout. Those signals are deliberately not guessed
to mean capacity, so this provider's late failure remains subject to its
ordinary timeout until it exposes a specific contract.

### Live-session-limit refusal

`session_limit_reached` is a `409` response with problem type
`session-limit-reached`. The request is valid and can succeed after the owner
pauses or ends enough sessions. It is not `429`: time does not clear this
condition, so no honest `Retry-After` exists and automatic retry is not useful.

```json
{
  "type": "https://spec.chrono-ai.fun/cma/1.0/problems/session-limit-reached",
  "title": "session limit reached",
  "status": 409,
  "detail": "3 of 3 sessions are already running for this owner; stop one before starting another",
  "code": "session_limit_reached",
  "limit": 3,
  "live": 3
}
```

`limit` and `live` are top-level integer fields so a client can act without
parsing the English `detail`. If an administrator lowered the limit beneath an
owner's current count, `live` may be greater than `limit`; the detail then says
that nothing new starts until fewer than `limit` sessions are running.

Creation is refused before an identifier is minted or a record is written.
Revive is refused before the paused record changes, so its history remains
readable and its state remains `idle`. The limit applies only where an owner is
asking for another runtime; reconciliation and rotation never reach it. See
the advisory allowance route in §3 for counting, non-retroactivity, and the
cross-replica enforcement boundary.

---

## 11. Administration

Everything an operator can see and do lives under `/api/v1/admin/`, one segment so the whole surface can be restricted in front of the service as well as inside it. Each endpoint is gated on its own capability, and no capability is implied by another.

| Permission | Capability | What it opens |
|---|---|---|
| `cma:admin:sessions` | read every session | `GET /api/v1/admin/sessions` |
| `cma:admin:users` | read the user directory | `GET /api/v1/admin/users` |
| `cma:admin:activity` | read the activity log | `GET /api/v1/admin/activity` |
| `cma:admin:sandboxes` | read the runtime inventory | `GET /api/v1/admin/runtimes` |
| `cma:admin:control` | stop or terminate any session | `POST /api/v1/admin/sessions/{id}/stop`, `.../terminate` |
| `cma:admin:settings` | configure the platform | `GET /api/v1/admin/settings`, `PUT /api/v1/admin/settings`, `PATCH /api/v1/admin/settings`, `POST /api/v1/admin/settings/reset` |

Reading and acting are separate grants on purpose: a role handed out so somebody can watch the service cannot also stop somebody's work.

`cma:admin:events` opens the durable event log of a session the caller does not own (`GET /api/v1/sessions/{id}/events`). It is not implied by `cma:admin:sessions`: knowing which sessions exist and reading everything one of them has ever done are different questions, and the second is the one the log answers.

Nothing under `/api/v1/admin/` is reachable on an everyday grant, including the two controls — a caller ending their own session does that at `/api/v1/sessions/{id}/terminate`. That is what lets the whole segment be restricted in front of the service without the gateway and the application disagreeing about who may call it.

**Naming people is not a second gate.** `GET /api/v1/admin/sessions` needs only `cma:admin:sessions`. A caller who also holds `cma:admin:users` gets `owner_label` on each row; one who does not gets the rows with `owner` alone. A view is never refused because a *second* capability is missing, or an empty list would stop meaning "there is nothing".

`GET /api/v1/admin/sessions` accepts the same exact `status`, clamped `limit`,
and opaque `cursor` parameters as the caller-owned session list. It defaults to
50 rows, never returns more than 200, remains a JSON array, and emits a next
`Link` only when another page exists. Unknown parameters are refused with
`400 invalid_query`.

**No read is unbounded.** The admin session collection uses cursor pages as
described above. `GET /api/v1/admin/users` and `GET /api/v1/admin/activity`
both take `limit`, both clamp it, and both default to 100.

**Refusals.** A caller without an operator capability gets `403` with `problem.type` ending `/not-an-operator` on the collections — they name no identifier, so there is nothing to protect by pretending it is missing. The two control endpoints keep the session surface's `404`, because they do address an identifier and a refusal must not confirm one exists. That `404` is byte-for-byte the one a session that does not exist produces: same status, same `type`, same `title`, same `detail`, same `code`. A body that differed would answer the question the status withholds.

### Platform settings

```http
GET   /api/v1/admin/settings
PUT   /api/v1/admin/settings
PATCH /api/v1/admin/settings
POST  /api/v1/admin/settings/reset
```

All four routes require `cma:admin:settings`, which appears to clients as the
`configure_platform` capability. The product wildcard `cma:*`
grants it. A signed-in caller without it receives `403` with a problem type
ending `/not-an-operator`; these routes name no protected identifier, so they
never disguise that refusal as `404`.

`GET` returns the complete current object, the revision a later write must
name, and the complete object replaced by the most recent write when one
exists:

```json
{
  "revision": 12,
  "settings": {
    "briefing": {
      "before": [
        "# Working in this session",
        "This session is already authenticated. Do not run `nyxid login`."
      ],
      "reach_heading": "## What you can reach",
      "reach_lead": "Use only the service slugs granted below.",
      "reach_close": "Calls to every other service are refused.",
      "reach_none": "No services were granted to this session.",
      "after": ["Continue with the owner's standing instructions."]
    },
    "deployment_capacity": {
      "cpu_millis": 20000,
      "memory_mib": 40960,
      "assumed_session_size": { "cpu_millis": 1000, "memory_mib": 1024 }
    },
    "pricing": { "cards": [] },
    "size_bounds": {
      "min_cpu_millis": 250,
      "max_cpu_millis": 4000,
      "min_memory_mib": 256,
      "max_memory_mib": 8192
    },
    "live_sessions": 4
  },
  "previous": {
    "briefing": {
      "before": ["# Working in this session"],
      "reach_heading": "## Granted services",
      "reach_lead": "The following service slugs are available.",
      "reach_close": "Other services remain unavailable.",
      "reach_none": "No services are available to this session.",
      "after": []
    },
    "deployment_capacity": {
      "cpu_millis": null,
      "memory_mib": null,
      "assumed_session_size": null
    },
    "pricing": { "cards": [] },
    "size_bounds": {
      "min_cpu_millis": 500,
      "max_cpu_millis": 3000,
      "min_memory_mib": 512,
      "max_memory_mib": 4096
    },
    "live_sessions": 3
  }
}
```

An untouched deployment answers at revision `0` with the shipped settings and
omits `previous`. `previous` has no revision of its own and is not an automatic
rollback: it is the full object available for inspection or recovery. To
restore its PATCH-editable values, send them in a PATCH together with the
current revision. Pricing history cannot be rolled back; a later rate is
represented by another sealed card.

`PUT` is a conditional replacement of the complete settings document. It is
the route used to configure pricing, because a price card is durable history
and is never treated as an ordinary field-level edit. Start from the complete
`settings` object returned by `GET`, retain every existing card byte-for-byte
and in order, and send it under `settings` with the revision that was read:

```http
PUT /api/v1/admin/settings
Content-Type: application/json

{
  "expected_revision": 12,
  "settings": {
    "briefing": {
      "before": ["# Working in this session"],
      "reach_heading": "## What you can reach",
      "reach_lead": "Use only the service slugs granted below.",
      "reach_close": "Calls to every other service are refused.",
      "reach_none": "No services were granted to this session.",
      "after": []
    },
    "deployment_capacity": {
      "cpu_millis": 20000,
      "memory_mib": 40960,
      "assumed_session_size": { "cpu_millis": 1000, "memory_mib": 1024 }
    },
    "pricing": {
      "cards": [
        {
          "id": "rate_9f2c4d71",
          "effective_from": "2026-08-23T05:00:00Z",
          "rate": {
            "cpu_micros_per_cpu_hour": 40000,
            "memory_micros_per_gib_hour": 5000
          },
          "retired_at": null
        }
      ]
    },
    "size_bounds": {
      "min_cpu_millis": 250,
      "max_cpu_millis": 4000,
      "min_memory_mib": 256,
      "max_memory_mib": 8192
    },
    "live_sessions": 4
  }
}
```

The only currency is USD. The two rate figures are integer micro-USD per
reserved CPU-hour and per reserved GiB-hour. An empty `cards` array means the
deployment has configured no price, not a zero rate. A legitimate price change
retires the current card and appends exactly one new card; the outgoing
`retired_at` and incoming `effective_from` are the same instant. Existing cards
cannot be removed, reordered, reopened, or have their timestamps or rates
edited. Such a document is refused as `422 price_list_not_append_only` and the
stored list remains unchanged.

`PATCH` is conditional and setting-granular. Every request includes
`expected_revision` and at least one setting. A setting that is omitted stays
unchanged; one that is present replaces that setting as a whole.

```http
PATCH /api/v1/admin/settings
Content-Type: application/json

{
  "expected_revision": 12,
  "briefing": {
    "before": [
      "# Working in this session",
      "This session is already authenticated. Do not run `nyxid login`."
    ],
    "reach_heading": "## What you can reach",
    "reach_lead": "Use only the service slugs granted below.",
    "reach_close": "Calls to every other service are refused.",
    "reach_none": "No services were granted to this session.",
    "after": ["Continue with the owner's standing instructions."]
  },
  "deployment_capacity": {
    "cpu_millis": 20000,
    "memory_mib": 40960,
    "assumed_session_size": { "cpu_millis": 1000, "memory_mib": 1024 }
  },
  "size_bounds": {
    "min_cpu_millis": 250,
    "max_cpu_millis": 4000,
    "min_memory_mib": 256,
    "max_memory_mib": 8192
  },
  "live_sessions": 5
}
```

A successful write increments the revision and returns the same complete shape
as `GET`. The object it replaced becomes `previous`:

```json
{
  "revision": 13,
  "settings": {
    "briefing": {
      "before": [
        "# Working in this session",
        "This session is already authenticated. Do not run `nyxid login`."
      ],
      "reach_heading": "## What you can reach",
      "reach_lead": "Use only the service slugs granted below.",
      "reach_close": "Calls to every other service are refused.",
      "reach_none": "No services were granted to this session.",
      "after": ["Continue with the owner's standing instructions."]
    },
    "deployment_capacity": {
      "cpu_millis": 20000,
      "memory_mib": 40960,
      "assumed_session_size": { "cpu_millis": 1000, "memory_mib": 1024 }
    },
    "pricing": { "cards": [] },
    "size_bounds": {
      "min_cpu_millis": 250,
      "max_cpu_millis": 4000,
      "min_memory_mib": 256,
      "max_memory_mib": 8192
    },
    "live_sessions": 5
  },
  "previous": {
    "briefing": {
      "before": [
        "# Working in this session",
        "This session is already authenticated. Do not run `nyxid login`."
      ],
      "reach_heading": "## What you can reach",
      "reach_lead": "Use only the service slugs granted below.",
      "reach_close": "Calls to every other service are refused.",
      "reach_none": "No services were granted to this session.",
      "after": ["Continue with the owner's standing instructions."]
    },
    "deployment_capacity": {
      "cpu_millis": 16000,
      "memory_mib": 32768,
      "assumed_session_size": { "cpu_millis": 1000, "memory_mib": 1024 }
    },
    "pricing": { "cards": [] },
    "size_bounds": {
      "min_cpu_millis": 250,
      "max_cpu_millis": 4000,
      "min_memory_mib": 256,
      "max_memory_mib": 8192
    },
    "live_sessions": 4
  }
}
```

The four settings accepted by `PATCH` have distinct replacement rules.
`pricing` is accepted only inside the complete document sent to `PUT`:

| Setting | PATCH rule |
|---|---|
| `briefing` | All six fields are required when it is named. `before` and `after` are ordered arrays of prose blocks and may be empty. The other four values are text and may deliberately be empty. `null` is refused; use the reset route for shipped prose. |
| `deployment_capacity` | The object replaces the whole declaration. `cpu_millis` and `memory_mib` are nullable non-negative `u64` totals; `null` leaves that dimension undeclared, while zero is an explicit ceiling. `assumed_session_size` is either `null` or a non-zero CPU/memory pair. The setting itself cannot be `null`. |
| `size_bounds` | All four non-zero `u32` values are required when it is named. CPU is in millicores and memory is in MiB. The range applies only when a session explicitly asks for a size; an unnamed size uses the deployment reservation even when that reservation lies outside these bounds. |
| `live_sessions` | A non-zero `u32` sets the per-subject limit. Explicit `null` means no limit. Omission means leave the current choice alone. Lowering it never stops existing work; it governs later create and revive asks, not reconciliation or rotation. |

Deployment capacity is a ledger over durable records whose desired state wants
a runtime. Pausing a session releases its reservation. Existing work is never
stopped when the declaration is lowered, and reconciliation and rotation are
never refused by it. If the new request names no size, or any counted record
has no size, and `assumed_session_size` is `null`, the sum cannot be computed
honestly: the entire capacity check is skipped and creation proceeds with an
operator warning. An unsized session is never counted as zero.

For example, this removes only the live-session limit and preserves the
briefing, deployment capacity, and size bounds:

```json
{ "expected_revision": 13, "live_sessions": null }
```

The granted-services list is deliberately absent from every settings payload.
When grants exist it is generated from the session's actual grants and inserted
between `reach_lead` and `reach_close` while the briefing is rendered. With no
grants, `reach_none` is rendered instead. A request cannot remove, replace, or
forge the generated list; it controls only the prose around it.

Only Codex currently reads the briefing document. Claude Code and the DeepSeek
Harness do not receive it. The setting is read when a runtime is provisioned;
changing it does not rewrite a runtime that is already running. Clearing the
prose is permitted, but doing so removes the guidance that the session is
already authenticated and that `nyxid login` will fail.

The empty body below is not a valid no-op and returns `422 invalid_request`:

```json
{ "expected_revision": 12 }
```

Unknown fields are also refused. A malformed settings body never echoes the
administrator's prose in its problem response.

Every write is optimistic. A PUT, PATCH, or reset whose `expected_revision` is no
longer current changes nothing and returns:

```http
HTTP/1.1 409 Conflict
Content-Type: application/problem+json

{
  "type": "https://spec.chrono-ai.fun/cma/1.0/problems/settings-conflict",
  "title": "settings conflict",
  "status": 409,
  "detail": "the platform's settings changed since they were read: these settings were changed by somebody else since you read them",
  "code": "settings_conflict"
}
```

On `settings_conflict`, read again and let the operator choose whether to load
the current values or retain their draft and explicitly repeat the write
against the new revision. A client must not silently retry with last-write-wins
semantics.

On a deployment with no settings store, `GET` still returns the shipped
defaults at revision `0`; `PUT`, `PATCH`, and reset return `503
settings_unavailable` rather than claiming an unwritten change succeeded. A
configured store that cannot be read or written uses the same 503 code. These
write routes return the 403 `not-an-operator` refusal described above when the
caller lacks `configure_platform`, and a stale revision always returns the 409
`settings_conflict` response without changing the stored document.

Reset restores only the shipped briefing. It preserves the current size bounds
and live-session limit, increments the revision, and records the complete
pre-reset object as `previous`:

```http
POST /api/v1/admin/settings/reset
Content-Type: application/json

{ "expected_revision": 13 }
```

Its successful response has the same complete `revision`, `settings`, and
`previous` shape as GET and PATCH. It is conditional in exactly the same way
and returns `409 settings_conflict` rather than resetting a revision the caller
did not inspect.

### Registered users

```http
GET /api/v1/admin/users?limit=100
```

```json
[{ "subject": "76fe9d91", "label": "Ada Lovelace", "display_name": "Ada Lovelace",
   "email": "ada@example.com", "first_seen": "2026-08-01T09:14:22Z",
   "last_seen": "2026-08-14T11:02:05Z" }]
```

Recorded on sign-in, since that is the only moment the identity provider's account details are in hand. `label` is what a client displays; `subject` is what it keys on. A person the provider released no details for carries no `label`, and a client falls back rather than presenting an opaque identifier as a name.

Most recently seen first, `limit` clamped to 500. Entries do not expire: one is a person rather than an event, and dropping one would lose the first time they were seen and make them look new on their return.

### Activity log

```http
GET /api/v1/admin/activity?actor=76fe9d91&limit=100
```

```json
[{ "id": "actv_0f3c…", "at": "2026-08-14T11:02:05Z", "actor": "76fe9d91",
   "action": "session_terminate",
   "target": { "kind": "session", "id": "sesn_01JD8Z3K7" },
   "outcome": "succeeded",
   "detail": { "runtime": "codex" } }]
```

Every request the control plane serves is recorded: who, when, what, with which parameters, and how it came out. `outcome` is one of `succeeded`, `denied` (turned away), `rejected` (sent something unusable), `failed` (our fault). `actor` is absent when the request carried no established identity — a refused caller is recorded precisely because they were refused.

Settings reads are recorded as `platform_settings_read`; successful `PUT` and
`PATCH` requests, and their refusals, are `platform_settings_write`. The reset
route retains its more specific `platform_settings_reset` action. A successful
write may carry `detail.setting`, a comma-joined list in declaration order of
the settings that actually changed, such as `"briefing,pricing"`. When pricing
changed it may also carry `detail.rate_card`, the identifier of the newly
in-force sealed card. A no-op write invents neither detail. These entries never
copy a rate, a monetary amount, or any administrator-authored briefing prose.

**`detail` never contains a secret.** What may be recorded is an allowlist in the core, and there is no key in it for an environment variable's value, a credential, a token, a cookie or a header. A bundle of variables is described by how many it carried, never by what they were. Newest first; `limit` is clamped by the service. Entries expire on a schedule the deployment sets (`CHRONO_ACTIVITY_RETENTION_DAYS`, ninety days by default).

Reading requires `cma:admin:activity`. There is no per-caller view: an ordinary caller cannot read the log at all, not even their own entries. What happened to their own session is in that session's event stream, which is theirs.

### Live runtimes

```http
GET /api/v1/admin/runtimes
```

```json
{ "running": 3, "fully_observed": true,
  "providers": [{ "kernel": "codex", "observable": true,
    "runtimes": [{ "runtime": "sbx_7Q2", "session": "sesn_01JD8Z3K7",
                   "owner": "76fe9d91", "status": "ready", "unaccounted": false }] }] }
```

The providers are asked what they are running and the answer is joined against the record, which is what supplies the session's owner and state. `unaccounted` marks a runtime nothing in the record is entitled to.

**A provider that cannot be asked is not a provider running nothing.** That provider carries `"observable": false` with a `reason` and **no `runtimes` array at all**, and the top-level `running` is omitted, because a total that silently excludes a provider is the same lie as an empty list. Clients must render this as "unknown", never as an idle namespace.

### Cross-owner session control

```http
POST /api/v1/admin/sessions/{id}/stop
POST /api/v1/admin/sessions/{id}/terminate
```

The same core operations the owner's own routes project, at an address that says who is taking them. `stop` stays reversible whoever asked for it; the session keeps its history and can be revived by its owner. Deleting is **not** here: running the service needs the ability to stop work that is costing money or misbehaving, and stopping is reversible where destroying somebody else's history is not.

Both are recorded in the activity log with the operator as `actor` and the session as `target`, and Layer 2 logs a warning whenever the caller is not the owner.

---

## 12. What the caller may do

A browser client asks who it is at `GET /auth/session`, and the answer says what that identity may do:

```json
{ "authenticated": true, "subject": "76fe9d91",
  "display_name": "Ada Lovelace", "email": "ada@example.com",
  "capabilities": ["read_all_sessions", "read_user_directory", "control_any_session",
                   "configure_platform"] }
```

The names are the core's capabilities, not the identity provider's permissions: a client is never given the provider's vocabulary, because role names, their nesting and their wildcards belong to whoever issues them and change without this service moving. An unauthenticated caller gets `401` with `{"authenticated": false}` and no `capabilities` key at all — there is no identity to have an entitlement.

**This is advisory.** It exists so a client can offer only what will be permitted, rather than offering everything and letting people discover refusals by hitting them. Every capability named here is checked again by the core on the request that uses it, so a client that ignored this list, or fabricated one, would gain nothing. Hiding a control is a courtesy; it is not access control.

A caller granted nothing is told `"capabilities": []`. The key is always present for an authenticated caller, so a client that reads an absent key as "unknown" and an empty one as "nothing" never has to guess which it is looking at.

Discovering entitlement by attempting the operation is deliberately **not** the intended route: every attempt is recorded in the activity log, so a client that probed on every page load would fill the record with refusals nobody asked for.
