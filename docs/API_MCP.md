# MCP Interface

**Priority 4 — for agent clients.**

CMA (ChronoAI Managed Agent) exposes itself as an **MCP server**, so any MCP-speaking agent — Claude Code, Codex, Cursor, or a custom one — can delegate work to a CMA session as a tool call, with no integration work beyond adding a server entry.

See [API_Overview.md](./API_Overview.md) for the core operation set and session model.

---

## 1. Naming collision — read this first

Two unrelated things in this system are called MCP:

| | Meaning |
|---|---|
| **This interface** | CMA **is** an MCP server. Agents call CMA. |
| **Kernel MCP servers** | A session's kernel connects **out** to MCP servers to use tools. |

This document describes the first. The second is session configuration, not a public interface.

---

## 2. Endpoint

```http
POST /api/v1/mcp
```

Streamable HTTP transport, JSON-RPC 2.0. Authenticated with a NyxID bearer token; every call is scoped to that subject.

MCP's current revision is stateless, so no session affinity is required at the load balancer.

---

## 3. Tools

Four tools. Deliberately few — an MCP client is delegating a task, not driving a UI.

### `chrono_start_session`

```json
{ "name": "chrono_start_session",
  "arguments": {
    "kernel": "codex",
    "task": "Add retry with backoff to the HTTP client and a test that proves it.",
    "workspace": { "type": "git", "url": "https://github.com/org/repo", "ref": "main" },
    "wait": true
  } }
```

Creates a session, delivers the task, and — with `wait: true` — blocks until the turn completes or an approval is required. Returns the session id, final status, and the agent's summary.

This interface is not built yet, and the shape above is a proposal rather than a description. One part of it is already known to be wrong: `kernel` is not a caller's to choose. Which one a session runs on comes from the environment profile it names or from what the deployment configured, and whoever implements this should drop the argument rather than invent a way to honour it.

`wait: false` returns the session id immediately for a caller that will poll.

### `chrono_send_input`

Deliver a follow-up turn to an existing session. Same wait semantics.

### `chrono_get_session`

Read status, recovery state, pending approvals, and usage. This is how a caller with `wait: false` polls.

### `chrono_resolve_approval`

Answer a pending permission request. See §4.

---

## 4. Approvals over MCP

A tool call is request/response, which appears to leave nowhere to deliver a mid-turn approval. MCP's current revision solves this properly, and CMA uses that mechanism rather than a workaround.

When the kernel blocks, the tool call returns an **input-required** result carrying the pending request and an opaque continuation token:

```
1. Client calls chrono_start_session
2. Kernel requests permission to run a command
3. Tool returns:
     resultType: "input_required"
     inputRequests: { "approval": { … request detail … } }
     requestState:  "<opaque>"
4. Client obtains a decision from its own user
5. Client calls chrono_resolve_approval with a NEW JSON-RPC id,
   echoing requestState verbatim
6. The session resumes; the original call's continuation returns
```

Rules a client must honour:

- **`requestState` is echoed verbatim.** It is opaque to the client, signed and bound to the subject and the originating request by CMA, and it expires.
- **The resolution uses a new JSON-RPC id.** It is a new request, not a response to the old one.
- A client that has not declared the relevant capability at `initialize` will not be sent an interactive request; it receives a typed error instead.

Between steps 3 and 5 the session is `idle` with `stop_reason: requires_action`, and that state is durable — a client that crashes can resolve the approval later, within the token's lifetime.

---

## 5. Operations projected

| Operation | Projected | How |
|---|---|---|
| `create_session` | ● | implicit in `chrono_start_session` |
| `send_input` | ● | `chrono_send_input` |
| `resolve_approval` | ● | input-required result plus `chrono_resolve_approval` |
| `stream_events` | — | no streaming; poll `chrono_get_session` |
| `interrupt` | — | not projected |
| `steer` | — | not projected |
| `resize_terminal` | — | meaningless here |
| `screen_snapshot` | — | not projected |
| `get_usage` | — | reported inside `chrono_get_session` rather than as its own operation |

This is the **narrowest** of the five interfaces, and that is intentional. An agent delegating a task wants a result, not a control surface. A caller needing streaming or steering should use [REST](./API_REST.md) or [AG-UI](./API_AGUI.md).

Invoking an unprojected operation returns a typed error naming the missing capability, never a silent no-op.

---

## 6. Capability declaration

Returned from the discovery call at initialisation:

```json
{ "capabilities": {
    "tools": { "listChanged": false },
    "completions": {},
    "extensions": {
      "ai.chrono/profile": {
        "classes": [
          "https://spec.chrono-ai.fun/cma/1.0/conf/mcp-core",
          "https://spec.chrono-ai.fun/cma/1.0/conf/approvals"
        ]
      },
      "ai.chrono/usage": { "perSession": true }
    }
  } }
```

chrono-specific capabilities live under the reverse-DNS prefix `ai.chrono/`, which ChronoAI owns — not under `experimental`, and never as bare top-level keys.

`resources` and `prompts` are not offered. `logging` is deprecated in MCP and is not implemented.

---

## 7. Errors

MCP defines a first-class error for "you invoked something your declared capabilities do not cover", carrying the missing capability identifiers in its data. CMA uses it, and mirrors its **shape** on every other interface — a machine-readable list of what is missing beats a bare status code.

| Condition | Result |
|---|---|
| Unknown tool | JSON-RPC method-not-found |
| Missing declared capability | capability error, listing what was required |
| Session not visible to the caller's subject | tool error; existence is not disclosed |
| Kernel blocked on permission | not an error — an input-required result |
| Control plane recovering | retryable error with backoff guidance |

---

## 8. Version policy

MCP is date-versioned and maintains a deprecation registry with removal dates. CMA pins a revision, supports one prior revision as a compatibility shim, and tracks the registry in CI so a scheduled removal fails a build rather than a customer's integration.

Clients should negotiate at `initialize` and never assume a revision.
