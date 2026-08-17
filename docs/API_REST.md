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

This is the form chrono-bot's own clients use. A server has no cookie jar, no redirect to follow and no browser to send to a login page, so the cookie is not available to it.

The token is checked with the provider rather than decoded here, which is what makes a withdrawn token stop working. The answer is cached for up to a minute, so revocation takes effect within that window rather than immediately.

Capabilities are the same either way: they come from the subject's roles as the provider reports them, so a client is never more privileged for having arrived over one route than the other.

A request with neither credential is `401`. A request whose credential is valid but insufficient is `403`.


| Concern | Rule |
|---|---|
| Base path | `/api/v1` |
| Auth | NyxID bearer token, or the chrono-bot session cookie for browser clients |
| Errors | RFC 9457 `application/problem+json` |
| Rate limiting | RFC 6585 `429` with RFC 9110 `Retry-After` |
| Pagination | RFC 8288 `Link` with `rel="next"` |
| Idempotency | `Idempotency-Key` required on every side-effecting `POST` |
| Timestamps | RFC 3339, UTC, always `Z` |
| Versioning | Additive within `/v1`. A breaking change mints `/v2`; `/v1` is never redefined |

Every response carries `X-Request-Id`. Include it in any support report.

---

## 2. Sessions

### Create

```http
POST /api/v1/sessions
Idempotency-Key: <client-generated-uuid>
Content-Type: application/json

{
  "llm_source": "chrono",
  "environment": "default",
  "workspace": { "type": "git", "url": "https://github.com/org/repo", "ref": "main" },
  "concurrent_input": "enqueue",
  "metadata": { "origin": "nyxid-chat" }
}
```

| Field | Required | Notes |
|---|---|---|
| `llm_source` | yes | `chrono` (brokered through NyxID) or `own` (the user's own account) |
| `environment` | no | Name of a pre-provisioned environment profile. **Never carries secret values** — it selects a profile provisioned out of band |
| `workspace` | no | Omit for an empty workspace |
| `concurrent_input` | no | `reject` \| `enqueue` \| `interrupt` \| `rollback`. Default `enqueue`. Immutable for the session's life |
| `metadata` | no | Opaque to chrono-bot; echoed back |

**A caller does not choose a kernel.** Which one a session runs on comes from the environment profile it names, or from what the deployment configured when it names none — one decision, made where a deployment already states it. There is no field for it, and that is deliberate: a request that could name one would be a request whose answer depended on who sent it.

`llm_source` and `environment` are **frozen at creation**, as is the kernel they resolve to. Changing any of them means a new session.

**`201 Created`**

```json
{
  "id": "sesn_01JD8Z3K7",
  "status": "provisioning",
  "recovery_state": { "state": "normal", "reason": null },
  "kernel": "codex",
  "llm_source": "chrono",
  "last_seq": 0,
  "created_at": "2026-08-12T09:14:22Z"
}
```

### Read, list, stop, revive, delete

```http
GET    /api/v1/sessions/{id}
GET    /api/v1/sessions?status=running&limit=50
POST   /api/v1/sessions/{id}/stop
POST   /api/v1/sessions/{id}/revive
GET    /api/v1/sessions/{id}/transcript
DELETE /api/v1/sessions/{id}
```

`stop` releases the runtime and keeps the session: it goes `idle`, stays listed, stays readable, and nothing is being paid for. `revive` gives it a runtime again under the same id, with the same history and the same options — reviving never mints a new session.

`DELETE` is terminal and irreversible. It is the only one of the three that loses anything, and it is not how you pause.

`transcript` returns everything the session's terminal has shown, as the byte stream a terminal renders — `application/octet-stream`, not JSON, because escaping an escape-sequence stream into a string is a decision the server has no business making. It works whether or not a runtime exists, which is how a stopped session stays readable.

### What a session may reach

A read carries `reach`: the services this session's credential may call on its owner's behalf.

```json
"reach": [
  { "id": "api-github", "name": "GitHub", "description": "Repositories and pull requests", "mandated": false },
  { "id": "nyxid", "name": "NyxID", "mandated": true }
]
```

This is the other half of a fact whose first half is `capabilities`. Capabilities are what a session can **do**; `reach` is what it can **touch**. A client showing one and not the other is describing half a session.

**It is the session's answer, not its profile's.** A session's reach is decided when it is provisioned and recorded on the session. A profile can be edited afterwards — widened, narrowed, or deleted — and the session keeps the scope it was provisioned with, so reading the profile to learn a running session's reach gives the wrong answer precisely when somebody has changed one. It survives a rotation for the same reason: a replaced runtime is the same session.

`mandated` marks what this deployment requires in every session, so a client can draw those entries as fixed rather than guessing which cannot be removed.

`name` and `description` come from the caller's own service catalogue and are **absent when it could not be asked**. A catalogue that is down costs the names and not the answer: the identifiers alone are still true, and reporting an error here would render a transient outage as a session that reaches nothing. See `docs/Services.md`.

**Never a credential.** `reach` carries identifiers and display names. What a session may reach is a fact about its configuration; what it holds is not, and no token, no expiry and nothing that could be used to fingerprint one appears here.

---

## 3. Input, interrupt, steer

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

**On the terminal plane, input is how approvals are answered and how a turn is steered.** That kernel asks on its own screen, and the answer is a keypress. There is no separate operation for it, and §5 says so where a client would look.

### Interrupt

```http
POST /api/v1/sessions/{id}/interrupt
```

Stops what the session is doing without ending it. On a conversation this cancels the turn in flight; on a terminal it delivers `SIGINT`.

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
| `interrupt` | **allowed** | a signal is not a keystroke — it does not enter the byte stream and cannot corrupt what is being drawn, and stopping a runaway command is exactly what an owner may need to do from elsewhere |

The rule is about *this* route, not about the terminal. The client holding the attachment drives it through its own connection and is unaffected: refusing writes at the operation itself would refuse that client its own keystrokes.

With no attachment live, all three operate the terminal directly.

---

## 4. Events

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

---

## 5. Approvals

```http
POST /api/v1/sessions/{id}/approvals/{request_id}
{ "decision": "allow" }
```

`decision` is `allow`, `deny`, or `abort`. Refusing one action and abandoning the turn are different intentions: `deny` refuses this and the turn carries on without it, `abort` refuses it and stops the turn. A client that meant the second and sent the first watches the agent try something else instead of stopping.

**Conversation sessions only.** On a terminal session this returns the typed capability error naming `conversation`: that kernel asks on its own screen and its answer is a keypress, so the operation to use is `POST /input` with the bytes the prompt is waiting for (§3). Inventing an approval route there would be a second way to say something the session cannot hear.

While an approval is pending the session is `idle` with `stop_reason: requires_action`, and the pending request ids are listed on the session object. That state survives a client disconnect and a control plane restart — a request waiting when you reconnect is still answerable. Answering one nothing is waiting for is refused rather than forwarded.

---

## 6. Terminal

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

## 7. Usage

```http
GET /api/v1/sessions/{id}/usage
```

```json
{ "input_tokens": 184203, "output_tokens": 9877, "context_used": 41204 }
```

Requires the `usage` capability. Not every kernel reports token counts, and one that does not returns the typed capability error rather than a plausible zero.

`input_tokens` and `output_tokens` are the **highest** totals this session's runtimes have reported, not the latest and not their sum. A session carried onto a replacement runtime is counted afresh by that runtime: taking the latest would show usage falling, and adding them would count the history the new runtime resumed from twice. `context_used` is how full the context is *now*, so it does fall — a conversation that was compacted genuinely holds less than it did.

Monetary accounting is not served here yet. When it is, amounts will be **integer strings in minor units** — `"412"` is $4.12 — never floats.

---

## 8. Capability declaration

```http
GET /api/v1/conformance
```

```json
{ "conformsTo": [
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/rest-core",
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/events-replay",
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/approvals",
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/steering",
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/interrupt",
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/terminal",
  "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/usage"
] }
```

Representations carry `Link: <…/conf/rest-core>; rel="profile"`, and the service advertises `Link: </openapi.json>; rel="service-desc"`.

The machine-readable contract is `GET /openapi.json` (OpenAPI 3.1), generated from code so it cannot drift.

---

## 9. Errors

```json
{
  "type": "https://spec.chrono-ai.fun/chrono-bot/1.0/problems/turn-in-progress",
  "title": "A turn is already in progress",
  "status": 409,
  "detail": "Session sesn_01JD8Z3K7 has an active turn and its concurrent_input policy is 'reject'.",
  "instance": "/api/v1/sessions/sesn_01JD8Z3K7/input",
  "request_id": "req_01JD8Z4M2"
}
```

| Status | When |
|---|---|
| `400` | Malformed request |
| `401` | Missing or invalid NyxID credential |
| `403` | Authenticated, but the session belongs to another subject |
| `404` | No such session, or not visible to this subject |
| `409` | Conflicts with current state — turn in progress, session terminated |
| `422` | Well-formed but semantically invalid — zero terminal dimension |
| `429` | Rate limited; observe `Retry-After` |
| `501` | The operation exists in the core but this interface does not project it |
| `503` | Control plane recovering or failing over; retry |

`501` responses name the missing capability in `detail`, so a client learns what is unavailable rather than guessing.

### The three service refusals

Starting a session against a service can fail for three different reasons, and a client that cannot tell them apart retries the one that will never succeed and gives up on the one that would have worked.

| `code` | Status | What it means | What to do |
|---|---|---|---|
| `service_not_granted` | `422` | The owner has not granted this service. `detail` names it. | Grant it in NyxID. **Retrying will not help.** |
| `service_catalogue_unavailable` | `503` | The catalogue could not be asked. The owner's services are fine. | **Retry.** Nothing is wrong with the session or the profile. |
| — | — | This deployment has no catalogue at all. | Nothing. It is not an error: profiles still store selections and sessions still honour them, there is simply nothing to choose from. |

Told apart by `code` and by status, never by prose — a client matching on a sentence breaks when somebody rewords one.

`service_catalogue_unavailable` is deliberately not `runtime_unavailable`: a caller told the runtime is unavailable goes and looks at the sandbox provider, which has nothing to do with it.

**The refusal is at provision, not at profile write.** A grant can be revoked between saving a profile and starting a session from it, so the write-time answer is friendlier and the provision-time answer is the true one. A profile naming a service the catalogue does not currently grant is stored, and logged as a warning; the session started from it is what gets refused.

**A catalogue that cannot be asked refuses nothing.** What a session may reach is recorded on the profile; the catalogue exists to help choose, not to authorise. Blocking session creation on NyxID being reachable would take the service down with it, and would do so for selections settled days ago.

See `docs/Services.md` for the whole path.

---

## 10. Administration

Everything an operator can see and do lives under `/api/v1/admin/`, one segment so the whole surface can be restricted in front of the service as well as inside it. Each endpoint is gated on its own capability, and no capability is implied by another.

| Permission | Capability | What it opens |
|---|---|---|
| `chrono-bot:admin:sessions` | read every session | `GET /api/v1/admin/sessions` |
| `chrono-bot:admin:users` | read the user directory | `GET /api/v1/admin/users` |
| `chrono-bot:admin:activity` | read the activity log | `GET /api/v1/admin/activity` |
| `chrono-bot:admin:sandboxes` | read the runtime inventory | `GET /api/v1/admin/runtimes` |
| `chrono-bot:admin:control` | stop or terminate any session | `POST /api/v1/admin/sessions/{id}/stop`, `.../terminate` |

Reading and acting are separate grants on purpose: a role handed out so somebody can watch the service cannot also stop somebody's work.

`chrono-bot:admin:events` opens the durable event log of a session the caller does not own (`GET /api/v1/sessions/{id}/events`). It is not implied by `chrono-bot:admin:sessions`: knowing which sessions exist and reading everything one of them has ever done are different questions, and the second is the one the log answers.

Nothing under `/api/v1/admin/` is reachable on an everyday grant, including the two controls — a caller ending their own session does that at `/api/v1/sessions/{id}/terminate`. That is what lets the whole segment be restricted in front of the service without the gateway and the application disagreeing about who may call it.

**Naming people is not a second gate.** `GET /api/v1/admin/sessions` needs only `chrono-bot:admin:sessions`. A caller who also holds `chrono-bot:admin:users` gets `owner_label` on each row; one who does not gets the rows with `owner` alone. A view is never refused because a *second* capability is missing, or an empty list would stop meaning "there is nothing".

**No read is unbounded.** `GET /api/v1/admin/users` and `GET /api/v1/admin/activity` both take `limit`, both clamp it, and both default to 100.

**Refusals.** A caller without an operator capability gets `403` with `problem.type` ending `/not-an-operator` on the collections — they name no identifier, so there is nothing to protect by pretending it is missing. The two control endpoints keep the session surface's `404`, because they do address an identifier and a refusal must not confirm one exists. That `404` is byte-for-byte the one a session that does not exist produces: same status, same `type`, same `title`, same `detail`, same `code`. A body that differed would answer the question the status withholds.

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

**`detail` never contains a secret.** What may be recorded is an allowlist in the core, and there is no key in it for an environment variable's value, a credential, a token, a cookie or a header. A bundle of variables is described by how many it carried, never by what they were. Newest first; `limit` is clamped by the service. Entries expire on a schedule the deployment sets (`CHRONO_ACTIVITY_RETENTION_DAYS`, ninety days by default).

Reading requires `chrono-bot:admin:activity`. There is no per-caller view: an ordinary caller cannot read the log at all, not even their own entries. What happened to their own session is in that session's event stream, which is theirs.

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

## 11. What the caller may do

A browser client asks who it is at `GET /auth/session`, and the answer says what that identity may do:

```json
{ "authenticated": true, "subject": "76fe9d91",
  "display_name": "Ada Lovelace", "email": "ada@example.com",
  "capabilities": ["read_all_sessions", "read_user_directory", "control_any_session"] }
```

The names are the core's capabilities, not the identity provider's permissions: a client is never given the provider's vocabulary, because role names, their nesting and their wildcards belong to whoever issues them and change without this service moving. An unauthenticated caller gets `401` with `{"authenticated": false}` and no `capabilities` key at all — there is no identity to have an entitlement.

**This is advisory.** It exists so a client can offer only what will be permitted, rather than offering everything and letting people discover refusals by hitting them. Every capability named here is checked again by the core on the request that uses it, so a client that ignored this list, or fabricated one, would gain nothing. Hiding a control is a courtesy; it is not access control.

A caller granted nothing is told `"capabilities": []`. The key is always present for an authenticated caller, so a client that reads an absent key as "unknown" and an empty one as "nothing" never has to guess which it is looking at.

Discovering entitlement by attempting the operation is deliberately **not** the intended route: every attempt is recorded in the activity log, so a client that probed on every page load would fill the record with refusals nobody asked for.
