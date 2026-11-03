# AG-UI Interface

AG-UI is CMA's primary live conversation interface. `threadId` identifies an
existing Agent, never a Sandbox, Workspace, Launch or a separately
created Chat. Provision and manage those resources through [REST](API_REST.md).
Replay/follow supports current organization ownership and sharing grants. Run
admission and the companion routes retain their documented personal scope.

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
Idempotency-Key: retained-run-1

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
Ordinary input always reserves a Message in the common command queue. Input
submitted while the Agent is Working waits for a later turn. Use the explicit
public control operations when steering the current turn is intended.

For a reply conditional on the latest completed answer, supply
`forwardedProps: {"answerContext":{"turn_sequence":3,"answer_sequence":5}}`.
Both sequences must be positive integers; the final assistant message must follow
its turn start. This extension applies only to a user message, never an approval.
It cannot override a profile or authorize a different Agent. New admission checks
the current successful final answer and rejects active, superseded or already
consumed answers atomically. A newer admitted message consumes that answer even
if it is subsequently cancelled or rejected. Invalid scope returns 422
`invalid_answer_context`; a stale answer returns 409 `agent_not_ready` with no
new admission. Preserve the exact text, scope and `Idempotency-Key` through an
uncertain response. Same-key recovery observes the original receipt under fresh
authority, even after its answer becomes stale. Different keys never deduplicate
by text. Ordinary clients omit `forwardedProps`.

CMA checks personal owner access and current credential authority, reserves a
common durable receipt, and lets the worker own readiness, checkpoint waits,
revival and dispatch. The stream opens only after that exact command has a
durable turn binding. It starts at its own turn even when other turns finished
while it waited. Disconnecting during admission or observation does not cancel
accepted work.

One optional `Idempotency-Key` accepts 1–256 bytes without control characters.
Retain the key with the same selected input or approval decision before sending.
Equivalent canonical intent and the same key share the existing authority,
Agent and command-kind namespace with the other common interfaces; differing
intent conflicts. A keyed retry reopens the same turn under current authority.
Omitting the key creates a separate immutable key for that HTTP invocation;
separate unkeyed attempts are separate commands, even if `runId` and text match.

Responses include `x-cma-command-id` and, for Message commands,
`x-cma-response-id`. These common IDs never replace the turn-derived `runId`.
The same headers accompany a prebinding error when the receipt is known.
`GET /api/v2/commands/{command_id}` observes the common command. After ten minutes
without an observed binding, `504 agent_command_observation_timeout` describes
uncertainty; it does not fail or cancel the accepted command. Committed queue
expiry instead returns `agent_command_queue_timeout`. Other committed unbound
refusals use typed authority, resource, target or execution problems, without
inventing `RUN_STARTED` or a terminal runtime event. Recover an uncertain keyed
POST with the same input/key, or read retained Agent events. The SDK never
resubmits automatically.

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
the entry from `/pendingApprovals/<escaped-id>`. Its common receipt binds to the
original target turn and creates no new Message response. The resumed stream
starts at that approval command's exact dispatch boundary, preserving events
that followed it while excluding earlier unrelated approval activity.

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

State paths include `/status`, `/title`, `/runtimeMetadata`, `/usage/inputTokens`,
`/usage/outputTokens`, `/usage/contextUsed`, `/usage/contextWindow`, `/mode`,
`/plan`, `/configOptions`, `/availableCommands`, `/signIn`, `/conversationGap`,
`/firstMessage` and `/pendingApprovals/<escaped-id>`. Approval ids use JSON
Pointer escaping. Usage values are accumulated readings, not token deltas.
`/firstMessage` carries `state` and `recoveryGeneration`; Agent readiness alone
does not prove that an admitted initial message has completed.

`/runtimeMetadata` replaces one snapshot with `workingDirectory`, `description`,
`descriptionSource` and `descriptionIsSetupPreview`. The source is `provider_title`,
`prompt_preview`, or null when unknown. Only a prompt preview positively marked
as setup should be excluded from a display title. Preserve explicit native
titles even when equal to setup text. Treat description and its provenance as
one update; this event does not mutate the configured Agent title. Shared reads
continue to withhold private runtime metadata.

During own-account startup, `/signIn` carries the provider challenge before
authentication completes. The durable `conversation_opened` event replaces
`/signIn` with `null`; it does not start a turn. Both updates retain their
durable cursors and replay after reconnect, with `runId` equal to `"0"` before
the first admitted turn.

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

## Organization And Shared Replay

`GET /api/v1/agui/events` requires current Agent history access. A narrow Agent
view grant permits only that Agent's events. It supplies no sibling, parent,
file, terminal or command access. Personal-owner replay keeps its existing frames.
Other readers receive the approved history projection: sign-in URLs/codes,
private correlation metadata, native conversation identifiers, configuration,
available commands and first-message internals are withheld. Runtime failure
messages are sanitized. A withheld durable record emits `STATE_DELTA` with an
empty `delta` array, its timestamp and its final cursor; apply no state change
and acknowledge that cursor normally. Live fragments have no durable cursor and
must still match the Agent, runtime generation, active turn and committed log.

The server checks current resource policy immediately before releasing each
buffered frame. Grant changes close the shared connection; a new GET evaluates
remaining grants. Membership evidence is bound to the exact authenticated
credential and expires within 60 seconds, including idle time and store IO.
The connection closes at expiry and stores no renewable request credential.
Reconnect from the last fully acknowledged durable cursor to obtain fresh
membership evidence. Removal, unknown roles or provider failures cannot extend
the old connection. An authorization failure before headers is a typed HTTP
problem; a failure after streaming starts ends observation without an Agent
failure event. The TypeScript replay client can reconnect within its configured
bounds; Rust callers reconnect explicitly. Personal-owner streams remain usable
when recipient grants change or the organization provider is unavailable.

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
