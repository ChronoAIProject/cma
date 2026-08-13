# REST Interface

**Priority 2 — for backend server clients.**

REST is the **superset interface**: it projects all nine core operations and is the reference realisation of the core session model. Every other interface is a projection of the same model; where a question about semantics is ambiguous elsewhere, this document is the answer.

See [API_Overview.md](./API_Overview.md) for the core operation set, session lifecycle, and event model.

---

## 1. Conventions

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
  "kernel": "codex",
  "llm_source": "chrono",
  "environment": "default",
  "workspace": { "type": "git", "url": "https://github.com/org/repo", "ref": "main" },
  "concurrent_input": "enqueue",
  "metadata": { "origin": "nyxid-chat" }
}
```

| Field | Required | Notes |
|---|---|---|
| `kernel` | yes | `codex` \| `claude_code` |
| `llm_source` | yes | `chrono` (brokered through NyxID) or `own` (the user's own account) |
| `environment` | no | Name of a pre-provisioned environment profile. **Never carries secret values** — it selects a profile provisioned out of band |
| `workspace` | no | Omit for an empty workspace |
| `concurrent_input` | no | `reject` \| `enqueue` \| `interrupt` \| `rollback`. Default `enqueue`. Immutable for the session's life |
| `metadata` | no | Opaque to chrono-bot; echoed back |

`llm_source`, `kernel`, and `environment` are **frozen at creation**. Changing any of them means a new session.

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

---

## 3. Input, interrupt, steer

```http
POST /api/v1/sessions/{id}/input
{ "text": "Refactor the auth module to use the new token type." }
```

```http
POST /api/v1/sessions/{id}/interrupt
POST /api/v1/sessions/{id}/steer
{ "text": "Stop — only touch files under src/auth/." }
```

`steer` redirects a turn already in progress. `interrupt` ends it. Sending input while a turn is in flight resolves according to the session's `concurrent_input` policy; on `reject` it returns `409` with `problem.type` ending `/turn-in-progress`.

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

`decision` is `allow` or `deny`; `reason` is optional on a deny and is surfaced to the agent so it can adapt.

While an approval is pending the session is `idle` with `stop_reason: requires_action`, and the pending request ids are listed on the session object. That state survives a client disconnect and a control plane restart — a request waiting when you reconnect is still answerable.

---

## 6. Terminal

```http
GET  /api/v1/sessions/{id}/screen          → current rendered screen
POST /api/v1/sessions/{id}/resize          { "cols": 120, "rows": 40 }
```

REST exposes these for clients that need terminal state without holding a stream. The live byte stream itself is the [PTY interface](./API_PTY.md).

`resize` with a zero dimension is rejected with `422`.

---

## 7. Usage

```http
GET /api/v1/sessions/{id}/usage
```

```json
{
  "tokens": { "input": 184203, "cached_input": 121004, "output": 9877, "reasoning": 4120 },
  "active_seconds": 1844,
  "cost": { "amount": "412", "currency": "USD" },
  "budget": { "max_cost": { "amount": "2500", "currency": "USD" } }
}
```

Monetary amounts are **integer strings in minor units** — `"412"` is $4.12. Never floats.

For `llm_source: "own"` the token figures are absent: the user's turns bill to their own account and chrono-bot cannot observe them. Container time is still metered.

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
