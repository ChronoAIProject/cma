# A2A Interface

**Priority 5 — for agent clients.**

**This interface is not built yet.** Nothing below is served today; it is the design the implementation will be held to, and the endpoint refuses with `501` until then.

A2A (Agent2Agent) lets another organisation's agent discover CMA (ChronoAI Managed Agent), delegate a coding task to it, and receive the result as artifacts. It is the interface for **agent-to-agent delegation across a trust boundary**, as distinct from [MCP](./API_MCP.md), where CMA is a tool inside someone else's agent loop.

A2A is the only protocol in the stack that is genuinely standardised — Apache-2.0, governed by the Linux Foundation, with a published specification and versioned bindings. That is why it is worth implementing despite being lowest priority.

See [API_Overview.md](./API_Overview.md) for the core operation set and session model.

---

## 1. Discovery

```http
GET /.well-known/agent-card.json
```

The Agent Card is the declaration surface. It states what CMA is, which transports it speaks, how to authenticate, and which capabilities it supports.

```json
{
  "name": "cma",
  "description": "Runs coding-agent kernels on ChronoAI infrastructure.",
  "url": "https://codex-api.chrono-ai.fun/api/v1/a2a",
  "capabilities": {
    "streaming": true,
    "pushNotifications": false,
    "extendedAgentCard": false,
    "extensions": [
      { "uri": "https://w3id.org/chrono/a2a/cma/v1",
        "description": "CMA session operations not expressible in core A2A",
        "required": false }
    ]
  },
  "skills": [
    { "id": "coding-task",
      "name": "Coding task",
      "description": "Executes a software engineering task in an isolated workspace and returns the diff.",
      "inputModes": ["text/plain"],
      "outputModes": ["text/plain", "application/json"] }
  ]
}
```

The card name is `cma`, and the extension URI above is its permanent v1 identifier. The earlier `chrono-bot` spellings appeared only in this unserved design: no Agent Card or A2A route has emitted them, so they receive no alias or compatibility window. A breaking extension change mints a new versioned URI rather than changing the meaning of this one.

Capability flags are set **truthfully**. Withholding an operation is legal only through a declared flag, and each has a specified error a client can act on. Silently not implementing something is not.

---

## 2. One interface, and why

The specification requires that every interface declared in a single Agent Card expose the **same** operation set. Declaring both a JSON-RPC and an HTTP+JSON interface would oblige them to be functionally identical.

CMA therefore **declares exactly one A2A interface**. CMA's REST API is a separate product surface and is deliberately *not* declared as an A2A binding — its operation set is larger, and conflating the two would violate that rule.

---

## 3. Mapping to the core model

| A2A concept | CMA |
|---|---|
| Task | A session, plus one delegated unit of work |
| Context | The session, across multiple tasks |
| Message | A turn |
| Artifact | Work product — a diff, a file, a report |
| Task state | Projected from session status and `stop_reason` |

### State mapping

| CMA | A2A task state |
|---|---|
| `provisioning`, `ready` | submitted |
| `running` | working |
| `idle` + `requires_action` | **input-required** |
| `terminated` + `end_turn` | completed |
| `terminated` + error | failed |
| `terminated` + `user_terminated` | canceled |

The gap worth naming: **CMA's `idle` — runtime reclaimed, session alive and revivable — has no A2A equivalent.** It is not `working` and not `completed`. CMA reports `working` and carries the true state in the extension metadata, because reporting `completed` for a session that will resume would be a lie a client acts on.

---

## 4. Operations projected

| Operation | Projected | How |
|---|---|---|
| `create_session` | ● | implicit — a task with no context creates one |
| `send_input` | ● | send a message |
| `stream_events` | ● | task status streaming, gated by `capabilities.streaming` |
| `resolve_approval` | ● | via the input-required state, §5 |
| `interrupt` | ● | cancel the task |
| `steer` | — | not projected; send a follow-up message instead |
| `resize_terminal` | — | meaningless for a delegating agent |
| `screen_snapshot` | — | not projected |
| `get_usage` | ● | returned as an artifact on request |

The core operations A2A cannot express live in **one** extension, identified by a permanent versioned URI. Extension data rides in the `metadata` map keyed by that URI. Extensions never add fields to core structures and never add enum values — a constraint that permanently rules out inventing a custom task state for `idle`.

A breaking change to the extension mints a **new URI**. There is no silent fallback to a prior version.

---

## 5. Approvals

Approvals map onto A2A's **input-required** state, which is the closest thing in the stack to a native fit:

```
1. Client submits a task
2. Kernel requests permission
3. Task transitions to input-required, with the request in the message
4. Client sends a message on the same task carrying the decision
5. Task returns to working
```

A task sitting in input-required is durable. A client that disconnects finds it still waiting.

---

## 6. Wire conventions

These are specification requirements, not CMA preferences, and clients depend on them:

- JSON field names are camelCase.
- Enum values serialise as `SCREAMING_SNAKE_CASE` strings — `"TASK_STATE_INPUT_REQUIRED"`, `"ROLE_USER"`.
- Timestamps are ISO 8601 in UTC.
- Pagination is cursor-based; the next-page token is **always present**, empty when the results are exhausted.
- Artifacts are omitted entirely, not returned empty, when not requested.
- Delete operations are idempotent.

---

## 7. Errors

Each capability flag has a specified error for the operation it gates, so a client learns *which* capability is missing rather than that something failed:

| Condition | Error |
|---|---|
| Streaming requested but not supported | unsupported-operation |
| Push notification configured but unsupported | push-notification-not-supported |
| Extended card requested but not configured | extended-card-not-configured |
| A required extension not echoed by the client | extension-support-required |

Task not found and task-not-cancelable use their standard codes. A task belonging to a different subject returns not-found; existence is not disclosed.

---

## 8. Card signing

The Agent Card may be signed. If ChronoAI signs it, note that capability booleans are presence-sensitive under canonicalisation: an explicit `false` and an omitted field are treated identically for capability gating but produce **different signatures**. Pin presence semantics, not only values, or signature verification will fail intermittently in a way that is painful to diagnose.

---

## 9. Conformance

The future implementation must serve all four ungated core operations — send message, get task, list tasks, and cancel task — in full before CMA may declare an A2A interface. There is no legal way to withhold any of them.

Before the interface may be declared, its capability-gated behaviour must be represented truthfully in the card and the specification's conformance suite must be added to CI. Tests gated on a capability CMA does not offer may then be skipped as the specification intends; ungated core operations must pass.
