# API Overview

chrono-bot exposes five public interfaces. Each is a **projection** of one core session model — never a separate product with its own semantics.

This document defines the shared vocabulary. The five interface documents define how each protocol expresses it:

| Priority | Interface | Client | Document |
|---|---|---|---|
| 1 | AG-UI | frontend clients | [API_AGUI.md](./API_AGUI.md) |
| 2 | REST | backend server clients | [API_REST.md](./API_REST.md) |
| 3 | PTY | CLI-like streaming clients | [API_PTY.md](./API_PTY.md) |
| 4 | MCP | agent clients | [API_MCP.md](./API_MCP.md) |
| 5 | A2A | agent clients | [API_A2A.md](./API_A2A.md) |

---

## 1. The core operation set

Nine semantic operations. Layer 2 owns them; Layer 1 projects them.

| Operation | Meaning |
|---|---|
| `create_session` | Provision a session bound to a kernel and an environment |
| `send_input` | Deliver a user turn to the session |
| `stream_events` | Receive the session's event stream, resumable from a cursor |
| `resolve_approval` | Answer a pending permission request |
| `interrupt` | Stop the in-flight turn |
| `steer` | Redirect a turn already in progress |
| `resize_terminal` | Inform the runtime its terminal geometry changed |
| `screen_snapshot` | Fetch the current rendered terminal screen |
| `get_usage` | Read token, time, and cost accounting for a session |

**The core is the union of what every interface and every kernel needs, never the intersection.** `resize_terminal` exists in exactly one interface and still belongs in the core.

### Which interface projects which operation

| Operation | AG-UI | REST | PTY | MCP | A2A |
|---|:---:|:---:|:---:|:---:|:---:|
| `create_session` | ● implicit | ● | — | ● implicit | ● implicit |
| `send_input` | ● | ● | ● | ● | ● |
| `stream_events` | ● | ● | ● | — | ● |
| `resolve_approval` | ● | ● | ● at the TUI | ● | ● |
| `interrupt` | ● | ● | ● | — | ● |
| `steer` | ● | ● | ● | — | — |
| `resize_terminal` | — | ● | ● | — | — |
| `screen_snapshot` | — | ● | ● | — | — |
| `get_usage` | ● | ● | — | — | ● |

**REST is the superset facade.** It has no structural constraint and implements every operation, so it is the reference realisation of the core. Every other interface is a projection of the same model, not of REST.

An interface exposing a subset is expected. An interface inventing an operation absent from the core is a defect.

---

## 2. Session model

### Lifecycle

```
provisioning ──► ready ──► running ⇄ idle ──► terminated
                    ▲          │
                    └─ rotating ┘
```

| State | Meaning |
|---|---|
| `provisioning` | Runtime is being created |
| `ready` | Runtime is up, awaiting input |
| `running` | A turn is in progress |
| `rotating` | **Being carried onto an updated runtime.** Not usable, not stopped, and coming back on its own |
| `idle` | **Runtime reclaimed; the session is intact and revivable.** Not an error, not termination |
| `terminated` | Terminal. Carries a `stop_reason` |

`idle` is the state most often misread. A session whose bot pod was reclaimed to save resources is `idle`, and it revives under the same session id.

`rotating` is the second. It means the runtime image has moved on and this session is being carried onto the current one: its conversation is captured, the sandbox is replaced, and the agent resumes from the capture. **Clients must refuse input while it lasts** — a keystroke sent now reaches a sandbox being torn down, so it would be accepted and then lost. It is not a failure, it needs no action from the owner, and it ends on its own.

### Runtime version

A session carries the runtime version serving it, how many times it has been carried onto a newer one, and the moment of the snapshot the current runtime resumed from:

```json
{ "runtime_version": "…/chrono-bot-runtime:8dbd315", "rotations": 1, "rotated_at": "2026-08-14T00:31:10Z" }
```

`rotated_at` is the honest part and clients should show it. Anything the agent did after that moment did not travel, so an owner who can see the boundary can recognise a gap rather than puzzle over one.

Sessions that have never been moved omit all three.

### Recovery state

Alongside lifecycle, a session projects a typed recovery state with a reason, matching the vocabulary used by fkst-hosted so operators read both control planes the same way:

`normal` · `idle` · `recovering` · `degraded` · `unknown` · `retired` · `invalid`

Two are routinely misread and clients must not treat either as failure:

- `recovering` with reason `runtime_absent` is **normal** during an idle-to-revive cycle.
- `unknown` with reason `runtime_observation_unavailable` means the control plane cannot observe the runtime right now — **not** that the session is dead.

### `stop_reason`

Every terminal or paused state carries one: `end_turn`, `user_terminated`, `idle_timeout`, `lifetime_cap`, `requires_action`, `revoked`, `budget_reached`, `error`.

`requires_action` means the session is waiting on the client — an approval, a tool result — and is **not** terminal.

---

## 3. Events

The event log is the single source of truth and the reason every interface can be a lossless projection.

- **Append-only**, with a **monotonic `seq` per session**. Not a global sequence.
- Resumable: `?starting_after=<seq>` replays from a cursor. One integer of server state; no `Last-Event-ID` plumbing.
- **Durable events are semantic.** Turn boundaries, committed messages, tool calls, approvals, status changes, usage, and the boundary where part of a conversation was lost.
- **Ephemeral events are deltas.** Token-level message pieces and terminal bytes are streamed for liveness and never persisted. A client that misses them recovers from the committed message, or from a screen snapshot.

Clients reconnect by opening the stream first, then reading history, then deduplicating on event id — never by assuming the stream replays.

---

## 4. Approvals

Every interface carries approvals through **one shared shape**: the runtime blocks, the session goes idle with `stop_reason: requires_action`, and the client answers by referencing the request id.

```
runtime blocks
  → event: approval_requested { request_id, action, detail }
  → session status: idle, stop_reason: requires_action
  → client answers: resolve_approval { request_id, allow | deny, reason? }
  → session resumes
```

There is no separate approvals resource. Approvals ride the event stream, so they inherit replay for free — a client that disconnects mid-approval finds the request waiting on reconnect.

---

## 5. Capability declaration

No specification defines a named conformance profile for agent services. chrono-bot therefore declares its own, and publishes it through each protocol's **native** mechanism rather than inventing a parallel one:

| Interface | Declaration surface |
|---|---|
| REST | `GET /conformance` returning `conformsTo[]` of class URIs |
| AG-UI | `GET /agui/capabilities`, plus `AgentCapabilities` on the SDK object |
| A2A | Agent Card `capabilities`, with chrono-specific operations in one extension |
| MCP | `ServerCapabilities` from `server/discover`, extensions under `ai.chrono/` |
| PTY | the `ready` control frame |

The same class URIs appear in every surface, so one vocabulary is visible everywhere. If a standard later emerges, the URIs change and the architecture does not.

**Invoking an operation an interface does not project returns a typed error naming the missing capability**, never a generic 404 or a silent no-op. MCP's `-32021` carrying `data.requiredCapabilities` is the shape every interface mirrors.

---

## 6. Rules that hold across all five

1. **One core.** Every interface calls Layer 2 and only Layer 2. No interface talks to a kernel adapter.
2. **No business logic in Layer 1.** An interface translates; it does not decide.
3. **Subsets are declared, not discovered.** A client can ask what an interface projects.
4. **Errors are typed and name what is missing.** Never a bare status code.
5. **Concurrent input has one answer.** A second turn arriving while one is in flight resolves identically on every interface. The policy is `reject`, `enqueue`, `interrupt`, or `rollback`, selected per session at creation and reported in its capability document.
6. **Identity is NyxID.** Every interface authenticates the same subject and scopes every operation to it. No interface has its own user model.
