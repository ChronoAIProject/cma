# API Overview

CMA (ChronoAI Managed Agent) exposes five public interfaces. Each is a **projection** of one core session model — never a separate product with its own semantics.

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
| `change_plane` | Move a session onto the other plane, as a rotation |

**The core is the union of what every interface and every kernel needs, never the intersection.** `resize_terminal` exists in exactly one interface and still belongs in the core.

### Which interface projects which operation

| Operation | AG-UI | REST | PTY | MCP | A2A |
|---|:---:|:---:|:---:|:---:|:---:|
| `create_session` | ● implicit | ● | — | ● implicit | ● implicit |
| `send_input` | ● | ● | ● | ● | ● |
| `stream_events` | ● | ● | ● | — | ● |
| `resolve_approval` | ● | ● | ● at the TUI | ● | ● |
| `interrupt` | ● | ● | ● | — | ● |
| `steer` | ● | ● | — | — | — |
| `resize_terminal` | — | ● | ● | — | — |
| `screen_snapshot` | — | ● | ● | — | — |
| `get_usage` | ● | ● | — | — | ● |
| `change_plane` | ○ observed | ● | ○ observed | — | — |

● served · ○ observed but not initiated · — absent

**`change_plane` is administration, not conversation.** AG-UI and PTY are told when it happens — a state delta carrying both planes and the snapshot moment, and a typed close frame naming where the session went — and neither offers a way to ask for it. That is a declared subset under rule 3, not an omission: a protocol whose vocabulary is runs and messages has no honest shape for "stop being a terminal", and a terminal that could talk itself out of existence would be a control with nothing left to draw its answer on. REST serves it, as the superset facade serves everything.

**REST is the superset facade.** It has no structural constraint and serves every operation, so it is the reference realisation of the core. Every other interface is a projection of the same model, not of REST.

A dot above means the interface projects the operation. Whether a *particular session* can be asked for it is a second question, answered by that session's capabilities: `steer`, `interrupt` and `get_usage` depend on what the kernel behind it actually offers, and `resize_terminal` and `screen_snapshot` on whether it has a terminal at all. An operation the session cannot honour is refused by name — never with a 404, and never accepted and dropped.

Capabilities are one half of what a session is; **reach** is the other. Capabilities say what a session can *do*; its reach says what it can *touch* — the services its credential may call on its owner's behalf. Both are decided when the session is created and recorded on it, so neither follows a later edit to the profile it was started from — and the granted set moves only when the owner moves the session between planes, which is a rotation and is written to the session's log, and REST projects both on the session object (`docs/API_REST.md`, `docs/Services.md`).

`send_input` is one operation with two projections, decided by the session rather than by the caller: keystrokes to a terminal, a prompt turn to a conversation. On the terminal plane it is also how that kernel's own on-screen approvals are answered and how a turn is steered, which is why `resolve_approval` reads "at the TUI" for PTY and why `steer` is not a separate call there.

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
{ "runtime_version": "…/cma-runtime:8dbd315", "rotations": 1, "rotated_at": "2026-08-14T00:31:10Z" }
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

- **Append-only**, with a **monotonic `seq` per session**. Not a global sequence. Per session, new events are committed and become readable in strictly increasing, contiguous `seq` order. A state-changing batch becomes durable atomically with its session record; retrying an uncertain result exposes neither a partial batch nor a repeated semantic event. A position already advertised by a deployment predating atomic commits remains reserved if its event was lost, so an upgrade never reuses a cursor a client may already hold.
- Resumable: `?starting_after=<seq>` replays from a cursor. One integer of server state; no `Last-Event-ID` plumbing.
- **Durable events are semantic.** The complete set, which is what `EventKind` holds and nothing more:

  | | |
  |---|---|
  | `session_created` · `progress` · `status_changed` | a session coming into being, and how it is getting on |
  | `turn_started` · `turn_completed` | turn boundaries |
  | `message_committed` | what the agent finished saying |
  | `tool_call_started` · `tool_call_updated` | what it did, and where that got to |
  | `usage_reported` | totals, not deltas — a lost total is corrected by the next one |
  | `conversation_gap` | the boundary where part of a conversation was dropped rather than recorded |
  | `runtime_lost` | the runtime under a session died, and why where the provider said |
  | `approval_requested` · `approval_resolved` | what it was blocked on, and how that was answered |
  | `terminal_attached` · `terminal_detached` · `terminal_resized` | who held the terminal, and at what size |

  A client must tolerate a discriminator it does not know: this set grows, and a control plane mid-rollout reads events a newer one wrote.

- **Ephemeral events are deltas.** Token-level message pieces and terminal bytes are streamed for liveness and never persisted — deliberately, because numbering them would punch permanent holes in the sequence a resuming client cursors on, and replaying them would show somebody their conversation being typed a second time. A client that misses them recovers from the committed message, or from a screen snapshot.

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

No specification defines a named conformance profile for agent services. CMA therefore declares its own, and publishes it through each protocol's **native** mechanism rather than inventing a parallel one:

| Interface | Declaration surface |
|---|---|
| REST | `GET /conformance` returning `conformsTo[]` of class URIs |
| AG-UI | `GET /agui/capabilities`, plus `AgentCapabilities` on the SDK object |
| A2A | Agent Card `capabilities`, with CMA-specific operations in one extension |
| MCP | `ServerCapabilities` from `server/discover`, extensions under `ai.chrono/` |
| PTY | the `ready` control frame |

The same class URIs appear in every surface, so one vocabulary is visible everywhere. If a standard later emerges, a new major contract version may adopt it without changing this version's identifiers in place.

The classes:

| Class | A session holds it when |
|---|---|
| `…/conf/core` | always — every session has a lifecycle, a log and a status |
| `…/conf/conversation` | its kernel holds a structured conversation and it was created for one |
| `…/conf/terminal` | its kernel renders a terminal and it was created for one |
| `…/conf/steering` | its kernel accepts input while a turn is in flight |
| `…/conf/interrupt` | its kernel can abandon a turn without ending the session |
| `…/conf/usage` | its kernel reports what it has spent |
| `…/conf/approvals` · `…/conf/events-replay` | the service does, for every session |

**Two claims, and they are not the same.** What an *interface* declares is what it could project for some session — the vocabulary a client must be ready for. What a *session* carries is its granted set, decided when it was created from what its kernel can actually do, and reported on the session itself. It changes only where its owner asks for `change_plane`, and that change is recorded rather than quiet. A service that renders terminals still cannot render one for a session that has none.

The two planes are exclusive: a session holds `conversation` or `terminal`, never both. Every interface dispatches on that to decide what an operation means, and a session claiming both would make "send input" ambiguous — keystrokes or a prompt, with no way to tell which was meant.

**Invoking an operation a session does not hold returns a typed error naming the missing capability**, never a generic 404 or a silent no-op. MCP's `-32021` carrying `data.requiredCapabilities` is the shape every interface mirrors.

A capability is declared only where it has been demonstrated against the kernel behind it. Kernels differ more than they look: one accepts input mid-turn and cannot be interrupted, another the reverse, and a third can do neither. Each absence is stated rather than discovered.

---

## 6. Rules that hold across all five

1. **One core.** Every interface calls Layer 2 and only Layer 2. No interface talks to a kernel adapter.
2. **No business logic in Layer 1.** An interface translates; it does not decide.
3. **Subsets are declared, not discovered.** A client can ask what an interface projects.
4. **Errors are typed and name what is missing.** Never a bare status code.
5. **Concurrent input has one answer.** A second turn arriving while one is in flight resolves identically on every interface. The policy is `reject`, `enqueue`, `interrupt`, or `rollback`, selected per session at creation and reported in its capability document.
6. **Identity is NyxID.** Every interface authenticates the same subject and scopes every operation to it. No interface has its own user model. What a session may reach on that subject's behalf, and how the credential gets there without entering the sandbox, is `docs/Services.md`.

---

## 7. Literal identifier stability

The conformance and problem URIs under `https://spec.chrono-ai.fun/cma/1.0/` and the AG-UI extension vocabulary under `cma:*` are protocol identifiers, not display names. Clients compare them literally. Their spelling is frozen for the 1.0 contract; changing either family requires a new major contract version and a compatibility plan.

Clients must still tolerate identifiers they do not recognise. Freezing existing names prevents an old meaning from moving, while accepting unknown names lets the vocabulary grow without making a newer server unreadable to an older client.

NyxID grants issued for this product use the canonical `cma:*` permission vocabulary. During the compatibility window, the control plane accepts each exact historical `chrono-bot:*` grant as an alias for its `cma:*` counterpart, including the product wildcard. This is an exact translation table, not prefix matching: an unknown or nearly matching grant still confers nothing. New roles and integrations must use `cma:*`; the old aliases remain only until provider inventory proves that no stored role or active authorization depends on them.

New per-session credential names use `cma:{deployment}:{session}` as their ownership record. Reclamation reads that form first and the historical `chrono-bot:{deployment}:{session}` form second because those credentials have no expiry backstop. `CHRONO_DEPLOYMENT_ID` is a state identity inside both forms, not a display name: existing deployments keep their explicit value unchanged, while the fallback for a new deployment is `cma`. The old prefix remains readable until complete provider listings for every session owner show that no matching key survives.

New browser sign-ins write only the `cma_session` cookie. During the compatibility window, authentication checks `cma_session` first and the historical `chrono_bot_session` second regardless of browser header order. Logout revokes every presented generation and expires both cookie names, so clearing the new cookie cannot expose a still-live old cookie on the next request. The old reader remains for one measured deployment cycle and is retired separately only after the maximum legacy-cookie lifetime and production observations show that it is no longer used.

New sandboxes carry the canonical correlation keys `cma/control-plane`, `cma/session`, and `cma/kernel`, alongside the exact historical `chrono-bot/*` counterparts with identical values. Reconciliation resolves the canonical session and control-plane values first and falls back independently to the historical values, so an old-only sandbox remains owned and cannot be mistaken for an orphan during the migration. The old readers remain until a complete, paginated provider inventory proves that no live pre-rename sandbox depends on them; runtime rotation supplies new sandboxes but is not itself that proof.

Tracing targets follow Rust source module paths rather than a wire identifier. The source rename therefore adopts `cma::*` for the control-plane crate without a `chrono_bot::*` alias. A repository-wide sweep found no explicit historical target in tracing calls, manifests, workflows, documentation, or runbooks: the deployment uses the broad `RUST_LOG=info` directive, and both binaries read `RUST_LOG` with an `info` fallback. No manifest transition or dual-read path is required. Any future target-specific filter must use the emitted canonical module target and be updated deliberately alongside a future source rename.

Browser-owned state writes only the canonical keys `cma:appearance`, `cma:terminal-theme`, `cma:view-mode`, `cma:pending-turn`, and `cma:rail-collapsed`. Persistent preferences read the canonical value first, fall back only to valid data under the corresponding historical `chrono-bot:*` key, and copy that value forward; the product terminal-theme ids similarly migrate from `chronobot-dark` and `chronobot-light` to `cma-dark` and `cma-light`. Pending turns remain per-tab in `sessionStorage`: consumption prefers valid canonical content and clears both names together, including when it falls back to historical content, so an old prompt is neither lost nor replayed. The historical readers remain separately gated until the browser compatibility window has been measured.

The A2A Agent Card name is `cma`, and CMA-specific task metadata is keyed by the permanent extension URI `https://w3id.org/chrono/a2a/cma/v1`. Both spellings are frozen before the interface is implemented. The earlier `chrono-bot` card name and extension URI existed only in an unserved design document, so no client or external state can depend on them and no compatibility alias is emitted. Any breaking extension change mints a new versioned URI.
