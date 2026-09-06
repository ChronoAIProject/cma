# AG-UI Interface

AG-UI is CMA's primary live conversation interface. `threadId` identifies an
existing caller-owned Agent, never a Sandbox, Workspace, Launch or a separately
created Chat. Provision and manage those resources through [REST](API_REST.md).
The same authentication and ownership rules apply to streaming requests.

## Routes

| Operation | Contract |
|---|---|
| `GET /api/v1/agui/capabilities` | Anonymous declaration of the currently composed feature set |
| `POST /api/v1/agui/run` | Submit the last user message or one approval answer to an existing Agent |
| `GET /api/v1/agui/events` | Replay/follow one Agent's durable and live events |
| `GET /api/v1/agui/agents/{id}/sandbox` | Owner-only Sandbox, Workspace, Worktree and first-message snapshot |
| `GET /api/v1/agui/launches/{id}/events` | Owner-only Launch progress SSE companion |

No AG-UI operation creates a Trigger, Launch, Sandbox or Agent implicitly.
Nonempty `forwardedProps` cannot override the Agent's frozen profile. Agent
terminal capabilities, WebSocket transport and snapshot events are absent.

## Run Admission

```http
POST /api/v1/agui/run
Content-Type: application/json
Accept: text/event-stream

{
  "threadId": "agt_example",
  "runId": "client-correlation-1",
  "messages": [{ "role": "user", "content": "Inspect the repository." }]
}
```

The optional request `runId` is a nonblank correlation value of at most 128
bytes. It is **not an idempotency key**, is not the durable turn id and does not
make a repeated POST safe. Output `runId` is the server's durable turn sequence
as a string. `messages` supplies the last user text, not a replacement history;
CMA retains prior history itself. Empty input without an approval is refused.
When the Agent is already working, user input is a steer operation.

CMA checks owner access and waits for an input-ready context, including Sandbox
revival where needed, before opening the stream. A failed admission returns a
normal typed HTTP problem. Once admission starts, loss of the HTTP connection
does not cancel it. Do not automatically repeat a run POST after a timeout or
lost response. Recover by reading the Agent and its durable events.

## Approval Answers

An approval request produces a `RUN_FINISHED` outcome with `type: "interrupt"`
and an `interrupts` array, followed by a `STATE_DELTA` adding that approval.
It is a pause for a decision, not evidence that the Agent has been stopped.

```json
{
  "threadId": "agt_example",
  "runId": "approval-correlation-1",
  "resume": [{
    "interruptId": "approval_example",
    "status": "resolved",
    "payload": { "decision": "allow" }
  }]
}
```

At most one resume is accepted. Decisions are `allow`, `deny` or `abort`.
Missing/unknown decisions and a status other than `resolved` are conservative
denials. Do not submit both a message and an approval expecting two operations:
when a resume exists it selects the approval path. Approval resolution removes
the entry from `/pendingApprovals/<escaped-id>`.

Explicit interruption is `POST /api/v1/agents/{id}/interrupt` through REST with
its required idempotency key. Closing a stream is not interruption.

## Event Frames

Success is `200 text/event-stream`, with unnamed SSE data frames. The event
kind is the JSON body's `type`, not SSE's `event` field. Every Agent frame
carries `threadId` and `runId`. Durable projections may carry `timestamp`.
Keepalive comments are not events.

| Event | Source and relevant fields |
|---|---|
| `RUN_STARTED` | Durable turn admission |
| `TEXT_MESSAGE_START` | `messageId`, role `user` or `assistant` |
| `TEXT_MESSAGE_CONTENT` | `messageId`, `delta`; committed complete text or an explicitly live delta |
| `TEXT_MESSAGE_END` | Committed message completion |
| `RUN_FINISHED` | Complete or interrupt outcome; approval requests carry interrupts |
| `RUN_ERROR` | Recorded turn failure, stopped Agent or deleted Agent; `code`, `message` |
| `TOOL_CALL_START` | `toolCallId`, `toolCallName`, optional `cma:toolKind` |
| `TOOL_CALL_END` | `toolCallId`, `cma:status` |
| `REASONING_MESSAGE_CONTENT` | Live-only reasoning text when the deployment advertises live frames |
| `STATE_DELTA` | JSON Patch-style updates to the closed state paths below |

State paths include `/status`, `/title`, `/usage/inputTokens`,
`/usage/outputTokens`, `/usage/contextUsed`, `/usage/contextWindow`, `/mode`,
`/plan`, `/configOptions`, `/availableCommands`, `/signIn`, `/conversationGap`,
`/firstMessage` and `/pendingApprovals/<escaped-id>`. Approval ids use JSON
Pointer escaping. Usage values are accumulated readings, not token deltas.
`/firstMessage` carries `state` and `recoveryGeneration`; Agent readiness alone
does not prove that an admitted initial message has completed.

The server does not emit a `STATE_SNAPSHOT`. Initialize from REST Agent detail,
transcript, usage and all durable event pages, then fold subsequent deltas.
Do not treat provider sign-in URLs or codes as general-purpose navigation or
persist them as durable client credentials.

## Durable Cursors And Live Output

```text
GET /api/v1/agui/events?thread_id=agt_example&starting_after=12&follow=true
```

`thread_id` is required. `starting_after` defaults to zero and accepts `after`
as an alias, not a second cursor. `follow` defaults to true; false gives a finite
replay. A cursor beyond the durable head returns `400 cursor_beyond_end` with
`head`. The Agent replay route does not consume `Last-Event-ID`: reconnect with
the explicit query cursor. The separate Launch companion below does consume it.

One log entry can project into several frames. Only its **last** frame has an
SSE `id`. A committed message therefore projects as START, CONTENT, then END
with the sequence on END. An approval projects FINISHED then STATE_DELTA with
the sequence on STATE_DELTA. Advance the durable cursor only after all those
frames have been applied. Intermediate no-id frames are not necessarily live.

Live frames are explicitly marked `cma:live: true` and may carry `cma:after`.
They have no durable id. Do not persist a live cursor or replay live text as a
second completed message. Durable completion supersedes the temporary live
message. Reasoning is not retained and cannot be reconstructed after a lost
connection. A `/conversationGap` notice records loss of live observations,
not invented missing durable text.

A run-scoped stream closes at its completion, failure, interruption, approval,
stop or deletion boundary. An unscoped follow connection can remain open across
turns. A follow disconnection is a transport fact, not a `RUN_ERROR`; reconnect
read-only from acknowledged progress with bounded backoff. If it remains
unavailable, preserve history and surface connection availability separately.

The TypeScript SDK's `agui.run` submits once. `agui.replay` bounds frame sizes,
reconnection attempts and callback delivery, resumes only acknowledged durable
progress, and accepts cancellation via `AbortSignal`. See the SDK reference for
its exact option names and error classes. The web client uses this same public
binding, with no private conversation transport.

## Read-Only Companions

The Sandbox companion supplies parent context and first-message state for an
owned Agent. It does not accept mutations or change its Agent's parentage.
Read Sandbox event history through REST to show recorded preparation, revival
and rotation progress rather than inferring steps from elapsed time.

The Launch companion streams `LaunchEventView` records as unnamed SSE frames,
with each durable sequence in `id`. It has no Agent `threadId` or `runId`.
It accepts `starting_after` (alias `after`), `limit`, `follow` and
`Last-Event-ID`; a valid header takes precedence over the query cursor. Unknown
or malformed parameters, duplicated cursor aliases and noncanonical decimal
cursors are refused. `follow=false` provides a finite page bounded by `limit`.
Use REST event pagination or follow for subsequent pages.

Launch progress is not a mutation acknowledgment from a guessed time interval.
Clients should refresh the owned Launch snapshot and only acknowledge a frame
when its event head has caught up. Service requirements, Start and cancel remain
REST operations. See [CMA Trigger](CMA_Trigger.md) for consent and recovery.

## Capability Declaration

Always read the composed capability document. It declares SSE streaming and
resumption, deltas and approvals; no terminal, WebSocket, state snapshots,
caller-defined tool arguments/results or full reasoning lifecycle is promised.
`cma:liveFrames` and `REASONING_MESSAGE_CONTENT` are enabled only when the
composition supplies a live event source. A protocol declaration does not prove
that every Agent's runtime can perform every operation.

Local projection tests prove event mapping, owner isolation, replay and
cancellation. Final image and production journey evidence is tracked separately
in [Operations](CMA_Operations.md), not inferred from the declaration.
