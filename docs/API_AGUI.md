# AG-UI Interface

**Priority 1 — for frontend clients.**

AG-UI is how a chat frontend renders a CMA (ChronoAI Managed Agent) session as a conversation: assistant prose, tool calls, reasoning, and approval prompts, streamed as they happen. It is the highest-priority interface because NyxID Chat and FKST Chat are the primary consumers.

See [API_Overview.md](./API_Overview.md) for the core operation set and session model.

---

## Authentication

Both stream-producing routes require an authenticated subject: `POST /api/v1/agui/run` and `GET /api/v1/agui/events`. A browser served on CMA's own origin signs in through the identity provider and sends the CMA session cookie. Every other caller sends the identity provider's access token in the HTTP header used by the REST interface:

```http
Authorization: Bearer <access token>
```

The token verification, revocation window, capabilities, and `401`/`403` distinction are the shared [REST authentication contract](./API_REST.md#authentication); AG-UI does not define another identity model. `GET /api/v1/agui/capabilities` is the exception: it is public interface discovery, requires no credential, and carries no session-specific authority.

There is no query-string credential for either AG-UI stream. In particular, `GET /api/v1/agui/events?...&ticket=<ticket>` is still unauthenticated without a cookie or Bearer header. The short-lived ticket is accepted only by `GET /api/v1/sessions/{id}/terminal` because the browser WebSocket API cannot set an authorization header. It is not an AG-UI credential and its scope is not widened here.

Native `EventSource` cannot set `Authorization`. It is therefore usable for AG-UI only by a same-origin browser whose session cookie is sent automatically. A cross-origin browser must use `fetch`, attach its Bearer token, and parse SSE incrementally from the response body rather than await the whole body:

```ts
const response = await fetch(
  `${cmaOrigin}/api/v1/agui/events?thread_id=${encodeURIComponent(sessionId)}`,
  {
    headers: {
      Accept: 'text/event-stream',
      Authorization: `Bearer ${accessToken}`,
    },
    credentials: 'omit',
  },
);
if (!response.ok || response.body === null) throw new Error(`AG-UI stream: ${response.status}`);
const reader = response.body.getReader();
```

The repository TypeScript SDK's [`createAguiTransport`](../sdk/typescript/src/agui/wire.ts) is the reference fetch/`ReadableStream` transport. Pair it with `AguiConnection`, which adds bounded-history ordering, cursor recovery, projection acknowledgement, and reconnection; product frontends should use the pair rather than reimplement those rules.

A cross-origin fetch is readable only when the browser application's exact origin is in the deployment's comma-separated `CHRONO_SERVER_CORS_ORIGIN` allowlist. CMA applies credentialed CORS to the complete public router, admits `Authorization` and `Last-Event-ID` on preflight, and deliberately has no wildcard-origin mode. An unlisted origin receives no `Access-Control-Allow-Origin`, so the browser reports a CORS or network failure before the Bearer-authenticated request reaches AG-UI. The operator setting and diagnostics are kept in the [cross-origin SDK deployment guide](./SDK.md#cross-origin-browser-deployments).

---

## 1. What AG-UI is not for

AG-UI carries **semantic events**. It cannot carry a terminal byte stream — there is no honest mapping from `\x1b[2J\x1b[H` to a message event. A client that needs the literal terminal uses the [PTY interface](./API_PTY.md).

Consequently AG-UI projects **seven** of the ten core operations. `resize_terminal` and `screen_snapshot` are absent, and their absence is declared rather than discovered.

`change_plane` is the third absence, and a different kind. AG-UI is **told** when a session's plane changes — see §3 — and offers no way to ask for one: this protocol's vocabulary is runs and messages, and there is no honest shape in it for "stop being a terminal". Ask over [REST](./API_REST.md); read the consequence here.

It projects **conversation sessions**. A session created in terminal mode carries no conversation capability, and a run against it is refused with the typed capability error naming `conversation` — before a stream opens, because a stream that opened and then said nothing would look like an agent thinking.

### The conformance table

One row per core operation, for a **conversation session**. It is the claim behind "a chat client can drive a CMA session", and it is narrower than that sentence sounds: two operations are terminal facts this protocol does not carry, and three depend on what the kernel behind the session offers.

| Operation | On a conversation session | If not |
|---|---|---|
| `create_session` | ● implicit — a run opens the thread | — |
| `send_input` | ● the prompt turn | — |
| `stream_events` | ● the SSE run stream, resumable from a cursor | — |
| `resolve_approval` | ● a new run carrying `resume` (§4) | — |
| `interrupt` | ● where the kernel offers it | `capability_unsupported: interrupt` |
| `steer` | ● under the session's `concurrent_input` policy | `capability_unsupported: steer` |
| `resize_terminal` | — not projected; a geometry is a terminal fact | change the session's plane |
| `screen_snapshot` | — not projected; a screen is a terminal fact | change the session's plane |
| `get_usage` | ● as `STATE_DELTA` on `/usage` | reported as absent, never as zero |
| `change_plane` | ○ observed as a `/planeChanges` delta | asked for over REST |

**No operation fails in any other way.** Silence, a stream that never resolves, and a generic error are each a defect of the same kind: rule 4 of `API_Overview.md` is that an error names what is missing, and a refusal a client cannot read is a refusal that did not happen. The rows above are driven by `web/src/chat/parity.test.tsx`, which fails if this table and the tests disagree.

This table is the per-session view of the same fact `API_Overview.md` §1 states per interface. Where they disagree, one of them is wrong.

---

## 2. Endpoints

```http
POST /api/v1/agui/run          → text/event-stream
GET  /api/v1/agui/events       → text/event-stream
GET  /api/v1/agui/capabilities → application/json
```

A run is one request. The response is an SSE stream of AG-UI events that ends when the turn ends.

`GET /api/v1/agui/events?thread_id=<session>&starting_after=<seq>` is the same event vocabulary, read rather than driven: a session's whole history and then everything after it, projected through the same mapping and never ending on its own. Every AG-UI event frame on this route carries the session id in both `threadId` and `runId`; a run response instead echoes that turn's accepted run id. Raw SSE keepalive comments are transport frames rather than AG-UI events and carry neither identifier. This route is what §7 means by recovering a dropped stream, and it exists because §7 used to send clients to `/api/v1/sessions/{id}/events` — a REST route, which answers in the core's vocabulary and not this one. A client that followed that advice received frames it could not read.

Both streaming responses emit a raw SSE comment at least every **15 seconds** while otherwise idle. The comment carries neither an SSE `id:` nor an `event:` name. A fetch-based SSE reader sees those comments and should treat roughly two intervals — about 30 seconds — with no bytes as a dead connection, then reconnect to the session stream with its last complete cursor as described in §7. This is the required transport for a cross-origin Bearer client and the recommended transport for any client that needs to enforce the read timeout.

The same-origin cookie clients that can use native `EventSource` do not receive SSE comments in application code. Such a client must detect a dead connection at the transport level rather than wait for a comment callback that can never occur. CMA deliberately does not emit application heartbeat events: doing so would add protocol vocabulary and require special exemptions from the durable-event cursor rules for frames that are only transport liveness.

**The sequence rides as the SSE `id`, not in the body, and only the final frame of a durable event's projection carries it.** Leading frames carry no `id`; receiving one does not yet mean the event arrived whole. A client resumes by handing back the last id it saw. The sequence is deliberately absent from the payload so nothing in the protocol's own shape has to be ignored.

### Running a turn

```http
POST /api/v1/agui/run
Content-Type: application/json

{
  "threadId": "sesn_01JD8Z3K7",
  "runId": "run_01JD9A2",
  "state": {},
  "messages": [
    { "id": "m_04", "role": "user", "content": "Refactor the auth module." }
  ],
  "tools": [],
  "context": [],
  "forwardedProps": { "environmentProfile": "envp_7dec1b0ca73c46b88121b259577bbe96" }
}
```

| Field | Meaning in CMA |
|---|---|
| `threadId` | **The session id.** A `threadId` that does not exist creates a session — this is how `create_session` is projected. The new session gets an identifier chosen by the service, not the one you sent, and every event on the run carries it: read `threadId` off the first event and use that from then on |
| `runId` | Client-generated id for this turn; echoed on every event of the run stream. It may be omitted or `null`, in which case CMA generates `run_` followed by 32 lowercase hexadecimal characters and echoes that value. An explicit value must contain a non-whitespace character and differ from the effective `threadId`; otherwise CMA returns HTTP 422 with code `invalid_request`. Frames read over `GET /api/v1/agui/events` carry the session's own id here — see §7 |
| `messages` | The conversation. The final user message is the turn's input |
| `forwardedProps` | Session creation parameters, honoured only when the thread is new and ignored thereafter. `kernel` is accepted and ignored in every case — which kernel a session runs on comes from the profile it names or from what the deployment configured, and no interface chooses one |
| `resume` | Present only when answering an interrupt — see §4 |

`forwardedProps` is ignored on an existing thread because what it sets is frozen at session creation. Changing it requires a new thread.

`environmentProfile` is the one property in it this service reads. `kernel` is accepted and ignored, as the table above says; `llmSource` was named here as though it were honoured and frozen, and has never existed anywhere in the service — the same phantom `docs/API_REST.md` carried as `llm_source`. Both are gone from the example above, so it now shows only what sending actually does something.

Unlike a body this service defines, an unrecognised property here is **not** refused. `forwardedProps` is AG-UI's own extension point, and a conforming client may forward props aimed at middleware or another consumer entirely; rejecting a run over one would break that. The absence is therefore declared rather than enforced, which is what `docs/API_Overview.md`'s "subsets are declared, not discovered" asks for. `POST /api/v1/sessions` takes the opposite approach for the opposite reason: that body is CMA's own, so an unknown field there is refused by name.

One consequence worth stating, because a client cannot discover it here: **this interface never reports which kernel a session ended up on.** No event and no state patch carries it. Over REST the created session echoes its `kernel` back, so a caller could notice; here there is nothing to notice, which is what makes the declaration above the only signal there is. `GET /api/v1/sessions/{id}` is where to look.

---

## 3. Events emitted

CMA emits a **subset** of the AG-UI event vocabulary. The authoritative list is the `events` map in the capability document; this table is its documented form.

| Group | Emitted | Carries |
|---|---|---|
| Run lifecycle | `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR` | turn boundaries and outcome |
| Assistant text | `TEXT_MESSAGE_START`, `TEXT_MESSAGE_CONTENT`, `TEXT_MESSAGE_END` | what the agent said |
| Reasoning | `REASONING_MESSAGE_CONTENT` | what the agent is working through — **live lane only, never durable** |
| Tool use | `TOOL_CALL_START`, `TOOL_CALL_ARGS`, `TOOL_CALL_END`, `TOOL_CALL_RESULT` | file edits, shell commands, MCP calls, their arguments and results |
| State | `STATE_DELTA` | usage, live resource readings, memory-pressure episodes, boundaries where conversation was lost, and plane changes |

### Canonical initial state

```json
{
  "usage": {
    "inputTokens": 0,
    "outputTokens": 0,
    "contextUsed": 0,
    "contextWindow": null
  },
  "mode": null,
  "plan": null,
  "configOptions": null,
  "availableCommands": [],
  "signIn": null,
  "gaps": [],
  "planeChanges": [],
  "contextCompactions": [],
  "runtimeLosses": [],
  "memoryPressure": null,
  "resources": null
}
```

`STATE_SNAPSHOT` is not emitted. Create this document once for each session and
apply every `STATE_DELTA` to it, in stream order, with RFC 6902 semantics. The
parents and replace targets are present deliberately: applying the same patches
to `{}` is invalid JSON Patch. A `null` value means no value has been reported;
in particular, a `null` `/usage/contextWindow` is "not stated", never zero.

For a replay from the beginning, discard the old state and start from this
document again. When resuming after a cursor, retain the state already folded
through that cursor and apply only the later deltas.

Not emitted **today**: `REASONING_START`, `REASONING_END`, the deprecated `THINKING_*` family, `STATE_SNAPSHOT`, and everything else in the enum. Each is declared `false` in the capability document rather than omitted from it, because omission reads as "this build has not heard of it" and a client sizing its handling needs the other claim.

A client must tolerate unknown event types rather than failing on them, and must not assume an event absent from this table will never appear — the set grows additively. That includes an unknown durable frame with no SSE `id`: an older client cannot know whether a future kind is a leading frame, so it retains the frame and groups it with the next numbered final just as it does a known leading frame.

### Reading history is a different request from watching

`GET /api/v1/agui/events` **holds the stream open across turns** — it is the session's stream, not one turn's, so it does not end on its own. That is what you want to watch a session, and it is a deadlock if you want history as a value: awaiting the response body settles only when the stream ends, so it never settles.

Send **`follow=false`** for a bounded read. It drains the log from your cursor and closes.

```
GET /api/v1/agui/events?thread_id=…&starting_after=41&follow=false   # history, ends
GET /api/v1/agui/events?thread_id=…                                  # watch, stays open
```

Absent means follow, so nothing written against the earlier behaviour changes.

### Frames are unnamed; the kind is in the body

Every frame on both streams arrives as the default SSE `message` type, with **no `event:` name**. The kind is `type` in the body, and that is what to dispatch on.

This is deliberate and it matters at the fetch-SSE parser boundary: pass every complete SSE data frame to the JSON decoder, then dispatch on the payload's `type`. Do not filter by SSE event name. The emitted set grows additively, so a transport that forwards only names it already knows silently loses every kind added after that client was written.

An older control plane may still name frames. A fetch-SSE parser should accept either shape, verify that a present non-default event name agrees with the body type, and dispatch on the body type in both cases.

### The session stream carries the live lane too

Both `GET /api/v1/agui/events` and the run stream carry it, and a client needs only one of them. **The session stream is the one to use**: it is held open across turns, so a client that posts a run and does not consume its response — which is the ordinary shape of a chat client — still sees everything happening.

Holding both would mean deciding for yourself how to interleave the durable frames and the live ones, and they describe one turn. Pick the session stream and there is nothing to interleave.

Liveness attaches when the session is running. A stream opened before that — which is what a client does the moment it has a session — attaches on its own once the runtime is up; nothing needs reopening.

### The run stream has two lanes, and one of them is not history

Everything above is the **durable** lane: projected from the log, with each projection's final frame carrying the SSE `id` a client resumes from. A leading frame without an id remains durable and does not carry `cma:live`.

The run stream also carries a **live** lane — what is happening right now. Those frames are marked and carry **no `id` at all**:

```json
{
  "type":         "TEXT_MESSAGE_CONTENT",
  "messageId":    "msg_live",
  "delta":        "Hel",
  "cma:live":  true
}
```

**Never resume from one.** It has no place in the log, so a cursor taken from it points at a position the log has never held, and everything between there and the real position is skipped on the next reconnect.

A raw fetch-SSE parser sees that this frame omitted the `id` field. Native `EventSource` does not: its `lastEventId` inherits the most recent id, which makes absence look like repetition. The marker keeps the distinction explicit on either transport. **A client must exclude `cma:live` frames from whatever it deduplicates and cursors on**, and only from that.

The lane also carries the agent's reasoning:

```json
{
  "type":         "REASONING_MESSAGE_CONTENT",
  "messageId":    "reasoning_live",
  "delta":        "the file is probably under src/",
  "cma:live":  true
}
```

**`REASONING_START` and `REASONING_END` are not emitted, and their absence is deliberate.** This service sees a stream of thoughts and has no view of where one begins or ends, so a bracket sent from here would be a guess. Bracket on the run's own lifecycle events instead — they are exact — and clear reasoning when the run ends. Nothing of it survives, which is the point: reasoning is never committed, so a reload shows none of it and must not.

`THINKING_START`, `THINKING_END` and `THINKING_TEXT_MESSAGE_*` are AG-UI's deprecated spelling of this family. They are declared `false` in the capability document rather than omitted, so a client written against the older names is told they are not coming rather than left waiting on them.

`msg_live` is the in-flight message's identity, and it is deliberately not the committed message's. The committed one arrives under `msg_<seq>`, and a client **replaces** the in-flight message with it rather than appending — otherwise the turn is drawn twice, once as it was typed and once as it was recorded.

Declared as `cma:liveFrames` in the capability document. Where it is false, or where a session's kernel offers no liveness, the durable lane is unaffected and the section below is the whole of what a client sees.

Resource readings are cursorless state on this lane too. Each reading is one
whole-object replace, never a set of field patches:

```json
{ "type": "STATE_DELTA", "cma:live": true, "delta": [{
  "op": "replace", "path": "/resources", "value": {
    "cpuMillisUsed": 340, "cpuMillisLimit": 1000,
    "memoryBytesUsed": 892211200, "memoryBytesLimit": 1073741824,
    "memoryBytesPeak": 930086912, "stallPermille": 12,
    "ceilingHits": 0, "allocationFailures": 0, "kills": 0,
    "receivedBytes": 18442, "sentBytes": 9120,
    "receivedBytesPerSecond": 420, "sentBytesPerSecond": 110,
    "intervalMs": 2000
  }
}] }
```

All fourteen keys are always present. An unread level, an absent limit, and a
rate with no predecessor are each `null`, never omitted and never zero. A
reading replaces its predecessor whole so a dropped frame self-heals on the
next one, and so `/resources` remains reconstructible without a
`STATE_SNAPSHOT`.

The conversation-liveness receiver and resource receiver are independent in
the service, so a two-second gauge cannot spend the lag budget reserved for a
message being typed. They share the same cursorless wire treatment once
merged onto this stream.

This lane alone cannot distinguish "no reading has arrived yet" from "this
session's current runtime cannot ever report one". That status changes at most
once during bring-up and does not earn a second stream path; call the public
`GET /api/v1/sessions/{id}/resources` point read once when the surface mounts
and again after a reconnect. A third-party frontend can make exactly the same
call as the reference client.

### Committed text arrives whole, not as tokens

On the durable lane, `TEXT_MESSAGE_CONTENT` carries the finished message rather than a stream of deltas. The token-level pieces are the live lane above, and they are deliberately not part of the durable log: numbering them would punch permanent holes in the sequence a resuming client cursors on, and replaying them would show somebody their conversation being typed a second time.

The consequence for a chat UI is worth stating plainly: **the log** gives you the message when the agent commits to it. If you want it while it is being written, read the live lane and expect nothing from it to survive a reload.

`messageId` is derived from the durable sequence, so reading the log twice yields the same identifier. A client deduplicating on it after a reconnect sees one message rather than two.

**`STATE_DELTA` is where `get_usage` lands.** Token counts arrive as RFC 6902 patches against `/usage`, so a chat UI can show spend without polling. The current reading is written at `/usage/contextUsed`; the ceiling it is drawn from is written at `/usage/contextWindow` only where the runtime states one. An absent ceiling means "not stated", never zero, and a later usage report that omits it does not erase one already stated.

Memory pressure is durable state folded from those ephemeral readings. An
opening edge replaces `/memoryPressure` with `usedBytes`, `limitBytes`,
`occupancyPermille`, and `since`, where `since` is the durable event's own
timestamp. A closing edge replaces the path with `null`, never removes it: a
client that joined after the open may not have the path, and RFC 6902 would
reject removing it. Neither edge ends the run; a squeeze is something to show
while the agent remains working, not a `RUN_ERROR`.

When the agent summarises the conversation to make room, a patch is appended to `/contextCompactions/-` with `was`, `now`, and `window` where a ceiling was stated. `window` is omitted rather than null where none is known. This is a durable boundary and precedes the lower `/usage/contextUsed` reading that revealed it, so a replay explains why the conversation became smaller instead of leaving its reader to infer that from the figures.

A stretch of conversation the runtime had to drop also arrives as a state patch, appended to `/gaps` — shown as a boundary rather than a stream that silently skips.

**A plane change lands there too**, appended to `/planeChanges`:

```json
{ "delta": [ { "op": "add", "path": "/planeChanges/-", "value": {
  "from": "terminal", "to": "conversation", "snapshotAt": "2026-08-18T09:14:22Z"
} } ] }
```

`snapshotAt` is the moment the conversation was captured, and it is where history stops being continuous: anything the agent did after it did not travel. Draw the boundary — a client that skips it silently leaves its reader to conclude the agent forgot something.

**A lost runtime is appended to `/runtimeLosses/-`:**

```json
{ "delta": [ { "op": "add", "path": "/runtimeLosses/-", "value": {
  "runtime": "runtime_01K35QJ0", "cause": "out_of_memory", "noticedAt": "2026-08-23T04:12:00Z"
} } ] }
```

`cause` is `out_of_memory`, `ended`, `failed`, `displaced`, or `unrecognised`, and is `null` where the service could establish no reason. `noticedAt` is when the service established that the runtime was lost, up to one reconciliation sweep after it actually stopped; it is not a claimed time of death. Do not draw the loss as the session having failed: a replacement runtime is started. The agent's conversation does **not** cross a runtime it lost, however, so it begins again from the terminal context that was captured rather than resuming that conversation.

**The move ends the run.** A session being carried onto another runtime, whether onto a newer image or onto the other plane, goes `rotating`, and a `RUN_ERROR` is emitted. Nothing is being lost: the run is over because its runtime is being torn down, and a stream that simply stopped could not be told apart from an agent taking its time. Start a new run when the session is usable again — the same session, the same conversation, resumed.

### `RUN_ERROR` covers two different things, and says which

The protocol has one event for a run ending badly, and two things end one: the agent's work failed, and the session is being carried onto another runtime. **Do not draw them the same way.**

```json
{
  "type":            "RUN_ERROR",
  "message":         "this session's runtime can no longer be what it is",
  "code":            "error",
  "capability":      "conversation",
  "cma:returns":  false
}
```

| Field | |
|---|---|
| `message` | what happened, in the kernel's own words. Always present. |
| `code` | the stop reason as this API spells it. Absent where the session did not stop for a reason it names. |
| `capability` | **the ability the session lacked**, present only when the failure was a refusal. Named rather than described: rule 4 of `API_Overview.md` asks errors to name what is missing, and a sentence that happens to contain the word does not satisfy that, because nothing downstream can read a sentence. Most failures lack nothing and carry no `capability` — a provider that was unreachable is not a session missing an ability. |
| `cma:returns` | **`true` when the session is coming back.** This is a handover, not a fault: the run ended because the runtime behind it is being torn down, and a new run starts on the same session once it is usable. Draw it as a boundary, not as a failure. |

A client that ignores `cma:returns` draws every rotation as a fault, and under §5 of `CLAUDE.md` a rotation is a routine consequence of a deployment. Treat its absence as `false`: that is the safer of the two to be wrong about, because a failure drawn as a handover tells somebody their session is returning when it is not.

### Tool kinds are the runtime's own word

`TOOL_CALL_START` carries `cma:toolKind`, and `TOOL_CALL_END` carries `cma:status`, both verbatim from the kernel. They are not mapped onto a fixed set: the vocabularies differ between kernels and grow without warning, and a session must not fail because one invented a category. Render what you recognise.

A durable tool-start record always projects `TOOL_CALL_START` and, when the record contains arguments, follows it with `TOOL_CALL_ARGS`. A durable tool update likewise projects `TOOL_CALL_END` and, when the record contains a result, follows it with `TOOL_CALL_RESULT`. Both frames in a pair carry the same `toolCallId`.

That optional follow-up makes `TOOL_CALL_START` and `TOOL_CALL_END` deliberately ambiguous on their own. If no follow-up exists, the start or end is the projection's final frame and carries the durable sequence as its SSE `id`. If `TOOL_CALL_ARGS` or `TOOL_CALL_RESULT` follows, the start or end carries no id and the follow-up carries it instead. The start/end body does not announce which case applies, so a client must accept both shapes, upsert by `toolCallId`, and advance its cursor only when the numbered final frame arrives.

**`STATE_DELTA` is where `get_usage` lands.** Token counts and cost arrive as state patches rather than a separate call, so a chat UI can show spend without polling.

---

## 4. Approvals — the shape that surprises people

AG-UI **ends the run to ask, and starts a new run to resume.** An approval is not a mid-stream prompt.

```
1. Codex requests permission to run a command
2. CMA ends the run:

   RUN_FINISHED {
     outcome: {
       type: "interrupt",
       interrupts: [{
         id: "apr_7Q2",
         reason: "cma:command_execution",
         message: "Run `rm -rf build/`?",
         responseSchema: { … },
         expiresAt: "2026-08-12T09:20:00Z"
       }]
     }
   }

3. Client renders approve / deny
4. Client POSTs a NEW run on the same threadId:

   { "threadId": "sesn_01JD8Z3K7", "runId": "run_01JD9A3",
     "resume": [{ "interruptId": "apr_7Q2", "status": "resolved",
                  "payload": { "decision": "allow" } }] }

5. The session resumes and streams the continuation
```

`reason` is `confirmation` where the runtime offered no description of what it wants, and `cma:<what the runtime called it>` where it did — `cma:command_execution`, `cma:file_write`, `cma:network_egress` and whatever else a kernel names. The prefix is what the protocol reserves for a vendor's own values, and the word after it is the runtime's rather than one this service chose: the sets differ per kernel and grow.

`payload.decision` is `allow`, `deny`, or `abort`. Refusing one action and abandoning the turn are different intentions, and a client that meant the second and sent the first watches the agent try something else instead of stopping. **A decision or status this service does not recognise is read as a refusal** — the one direction this can safely fail in, because the alternative is granting on a word it did not understand.

Between step 2 and step 4 the session is `idle` with `stop_reason: requires_action`. **That state is durable.** A client that closes the tab and returns finds the interrupt still pending and answerable; it does not need to have held the stream.

`status: "cancelled"` on resume is a deny.

---

## 5. Interrupt and steer

- **Interrupt** — `POST /api/v1/sessions/{threadId}/interrupt`.

  **A dropped stream does not cancel anything.** This was specified the other way round — "abort the HTTP request and CMA cancels the turn" — and that is wrong for two reasons that only became clear once the stream existed.

  A connection that drops because a laptop slept is indistinguishable from one a person closed on purpose. Reading the first as a cancellation ends somebody's turn because their network blinked, which §5 of `CLAUDE.md` forbids in the strongest terms it has: a session is not interrupted by anything its owner did not ask for. And it contradicts §7 of this document, which tells a client to recover a dropped stream by reading the durable log — advice that only makes sense if the work is still running.

  So a dropped stream is a client that stopped watching. The turn continues, its events land in the log, and reconnecting through the event stream finds them. Interrupting is an act, and it has a route.

  Interrupting requires the `interrupt` capability. Not every kernel has one — a wire can offer a prompt with no way to withdraw it — and a session whose kernel cannot is refused by name rather than left waiting for something that will not happen.

- **Steer** — POST a new run on the same `threadId` while one is in flight. What happens is governed by the session's `concurrent_input` policy (`reject` · `enqueue` · `interrupt` · `rollback`), fixed at session creation and reported in the capability document. On `reject`, the second run returns `409` before opening a stream.

---

## 6. Capability document

```http
GET /api/v1/agui/capabilities
```

```json
{
  "protocol": "ag-ui",
  "transports": ["sse"],
  "events": {
    "RUN_STARTED": true, "RUN_FINISHED": true, "RUN_ERROR": true,
    "TEXT_MESSAGE_START": true, "TEXT_MESSAGE_CONTENT": true, "TEXT_MESSAGE_END": true,
    "TOOL_CALL_START": true, "TOOL_CALL_ARGS": true, "TOOL_CALL_END": true,
    "TOOL_CALL_RESULT": true, "THINKING_START": false, "THINKING_END": false,
    "STATE_SNAPSHOT": false, "STATE_DELTA": true
  },
  "features": {
    "transport":      { "streaming": true, "resumable": true,
                        "websocket": false, "pushNotifications": false },
    "state":          { "snapshots": false, "deltas": true, "persistentState": true },
    "humanInTheLoop": { "supported": true, "approvals": true, "interrupts": true,
                        "approveWithEdits": false, "interventions": false },
    "execution":      { "codeExecution": true, "sandboxed": true },
    "custom":         { "cma:terminal": false, "cma:usage": true,
                        "cma:resources": true,
                        "cma:changePlane": false, "cma:planeChangeObserved": true }
  },
  "conformsTo": [
    "https://spec.chrono-ai.fun/cma/1.0/conf/agui-core",
    "https://spec.chrono-ai.fun/cma/1.0/conf/approvals"
  ]
}
```

`websocket: false` is deliberate. AG-UI here rides SSE; the terminal has its own WebSocket and is not an AG-UI transport.

The `events` map is the machine-readable form of §3 and is the contract. Read it rather than assuming.

---

## 7. Implementation notes for clients

- **Tolerate unknown events.** The emitted set grows additively; a client that throws on an unrecognised type will break on a routine release.
- **Do not treat `RUN_FINISHED` as "session over."** Check `outcome`. An interrupt outcome means the session is alive and waiting for you.
- **A dropped stream is not a cancellation.** The turn keeps running. Recover by reading the log, not by re-posting the run — and if you meant to stop it, say so with the interrupt route (§5).
- **Reconnect by reading, not by retrying the run.** A dropped stream is recovered with `GET /api/v1/agui/events?thread_id=<session>&starting_after=<seq>`, not by POSTing the run again — which would start a second turn. Read the cursor off the last SSE `id`; it is not in the body.
- **A cursor beyond the log's head is refused before an SSE stream opens.** The response is HTTP 400 `application/problem+json`, with type `https://spec.chrono-ai.fun/cma/1.0/problems/cursor-beyond-end` and code `cursor_beyond_end`. Its detail names both the offered cursor and the current durable head, and its numeric `head` field is the position a client can resynchronise from. A cursor at or below the head remains valid; one exactly at the head opens a followed stream and waits for the next durable event.
- **Native `EventSource` automatic reconnect replays from its original query cursor.** This endpoint deliberately ignores the browser's `Last-Event-ID` request header, so the retry does not advance `starting_after`. Deduplication of complete durable projections by their SSE `id` is therefore mandatory for this client class, even when the client also closes and reconstructs the source as described below.
- **An SSE `id` acknowledges a whole durable projection.** Only its final frame carries the event sequence. If the connection drops after an earlier frame, the last id still names the preceding durable event, so replay sends the interrupted projection again from its first frame. Group the prefixes with their next numbered final and deduplicate that complete projection by the final id. Do not deduplicate prefixes only by `messageId`, `toolCallId`, or kind: one tool call can have several distinct durable updates, and future unknown kinds may also be prefixes. Advance the stored cursor only after the complete group was accepted.

  A fetch-SSE reader can observe the current frame's actual `id` field and uses this procedure:

  1. Keep one acknowledged cursor for replay and deduplication. Put it in the `starting_after` query whenever constructing the followed stream; the query is authoritative even when the SDK also sends `Last-Event-ID`.
  2. Open that followed stream before requesting bounded history with the same cursor and `follow=false`. Queue followed frames until history has been accepted, then drain the queue. This closes the gap in which an event could otherwise be committed after the history read but before the live connection opened.
  3. Deliver a `cma:live` frame without putting it in the durable projection buffer and without changing the cursor. It has no durable position to acknowledge.
  4. For any other frame with no `id`, buffer it as a projection prefix. An explicit id must be a positive safe integer and marks the projection's final frame. Combine that final with the buffered prefixes, deduplicate the complete group by the final id, and clear the buffer.
  5. Advance the cursor only after the application accepts every frame in the complete group, and never move it backwards. On a drop, discard any incomplete prefix buffer, retain completed-projection deduplication state, and construct a new fetch from the latest acknowledged cursor. Never retry the POSTed run.

  Parsing the raw fields removes the inherited-`lastEventId` ambiguity native `EventSource` creates. The SDK's fetch transport reconstructs each request with its current cursor, while `AguiConnection` performs the open-stream-before-history ordering, projection grouping, acknowledgement, deduplication, and retry policy.
- **The events route identifies replay, not a run.** Route provenance is authoritative: every AG-UI event frame in both its initial history and followed tail carries the effective `threadId` as `runId`, because no one run owns the whole session history. The run route rejects explicit blank identifiers and `runId == threadId`, and its generated identifiers are also kept distinct, so equality is an exclusive replay-envelope signature on accepted responses. Do not derive conversation or per-run rendering state from it. The repository TypeScript SDK follows this rule: `AguiConnection` assembles each complete durable projection around its numbered final frame, preserves an unacknowledged projection for exact retry, and advances the cursor only after every callback accepts it; the conversation fold ignores `runId`, so run-stream and replay envelopes render the same conversation.
- **`STATE_DELTA` patches, it does not replace.** Apply deltas in order to the canonical initial state defined in §3; no `STATE_SNAPSHOT` arrives.

---

## 8. Stability

AG-UI is pre-1.0 and single-vendor, with no protocol version distinct from its package semver. CMA therefore versions **its own** projection through the `conformsTo` URIs above, and never claims conformance to an AG-UI version — no such version exists to cite.

If AG-UI makes a breaking change, this interface's `conformsTo` URIs change and the core session model does not.
