# AG-UI Interface

**Priority 1 — for frontend clients.**

AG-UI is how a chat frontend renders a chrono-bot session as a conversation: assistant prose, tool calls, reasoning, and approval prompts, streamed as they happen. It is the highest-priority interface because NyxID Chat and FKST Chat are the primary consumers.

See [API_Overview.md](./API_Overview.md) for the core operation set and session model.

---

## 1. What AG-UI is not for

AG-UI carries **semantic events**. It cannot carry a terminal byte stream — there is no honest mapping from `\x1b[2J\x1b[H` to a message event. A client that needs the literal terminal uses the [PTY interface](./API_PTY.md).

Consequently AG-UI projects **seven** of the ten core operations. `resize_terminal` and `screen_snapshot` are absent, and their absence is declared rather than discovered.

`change_plane` is the third absence, and a different kind. AG-UI is **told** when a session's plane changes — see §3 — and offers no way to ask for one: this protocol's vocabulary is runs and messages, and there is no honest shape in it for "stop being a terminal". Ask over [REST](./API_REST.md); read the consequence here.

It projects **conversation sessions**. A session created in terminal mode carries no conversation capability, and a run against it is refused with the typed capability error naming `conversation` — before a stream opens, because a stream that opened and then said nothing would look like an agent thinking.

### The conformance table

One row per core operation, for a **conversation session**. It is the claim behind "a chat client can drive a chrono-bot session", and it is narrower than that sentence sounds: two operations are terminal facts this protocol does not carry, and three depend on what the kernel behind the session offers.

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

`GET /api/v1/agui/events?thread_id=<session>&starting_after=<seq>` is the same events, read rather than driven: a session's whole history and then everything after it, projected identically and never ending on its own. It is what §7 means by recovering a dropped stream, and it exists because §7 used to send clients to `/api/v1/sessions/{id}/events` — a REST route, which answers in the core's vocabulary and not this one. A client that followed that advice received frames it could not read.

**The sequence rides as the SSE `id`, not in the body.** A client resumes by handing back the last id it saw, and deduplicates on it. It is deliberately absent from the payload so nothing in the protocol's own shape has to be ignored.

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
  "forwardedProps": { "kernel": "codex", "llmSource": "chrono" }
}
```

| Field | Meaning in chrono-bot |
|---|---|
| `threadId` | **The session id.** A `threadId` that does not exist creates a session — this is how `create_session` is projected. The new session gets an identifier chosen by the service, not the one you sent, and every event on the run carries it: read `threadId` off the first event and use that from then on |
| `runId` | Client-generated id for this turn; echoed on every event |
| `messages` | The conversation. The final user message is the turn's input |
| `forwardedProps` | Session creation parameters, honoured only when the thread is new and ignored thereafter. `kernel` is accepted and ignored in every case — which kernel a session runs on comes from the profile it names or from what the deployment configured, and no interface chooses one |
| `resume` | Present only when answering an interrupt — see §4 |

`forwardedProps` is ignored on an existing thread because `kernel` and `llmSource` are frozen at session creation. Changing them requires a new thread.

---

## 3. Events emitted

chrono-bot emits a **subset** of the AG-UI event vocabulary. The authoritative list is the `events` map in the capability document; this table is its documented form.

| Group | Emitted | Carries |
|---|---|---|
| Run lifecycle | `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR` | turn boundaries and outcome |
| Assistant text | `TEXT_MESSAGE_START`, `TEXT_MESSAGE_CONTENT`, `TEXT_MESSAGE_END` | what the agent said |
| Reasoning | `REASONING_MESSAGE_CONTENT` | what the agent is working through — **live lane only, never durable** |
| Tool use | `TOOL_CALL_START`, `TOOL_CALL_END` | file edits, shell commands, MCP calls |
| State | `STATE_DELTA` | usage, boundaries where conversation was lost, and plane changes |

Not emitted **today**: `TOOL_CALL_ARGS`, `TOOL_CALL_RESULT`, `REASONING_START`, `REASONING_END`, the deprecated `THINKING_*` family, `STATE_SNAPSHOT`, and everything else in the enum. Each is declared `false` in the capability document rather than omitted from it, because omission reads as "this build has not heard of it" and a client sizing its handling needs the other claim. They arrive when the durable log carries what they would need: the core records that a tool call started and where it got to, not the arguments it was given or what it returned.

A client must tolerate unknown event types rather than failing on them, and must not assume an event absent from this table will never appear — the set grows additively.

### The run stream has two lanes, and one of them is not history

Everything above is the **durable** lane: projected from the log, carrying an SSE `id`, and what a client resumes from.

The run stream also carries a **live** lane — what is happening right now. Those frames are marked and carry **no `id` at all**:

```json
{
  "type":         "TEXT_MESSAGE_CONTENT",
  "messageId":    "msg_live",
  "delta":        "Hel",
  "chrono:live":  true
}
```

**Never resume from one.** It has no place in the log, so a cursor taken from it points at a position the log has never held, and everything between there and the real position is skipped on the next reconnect.

The marker is sent because you cannot otherwise see the absence. `EventSource` reports `lastEventId` as the last id it was *given*, so a frame sent without one is indistinguishable from a frame repeating the previous id — and a client deduplicating on that drops every live frame in silence. **A client must exclude `chrono:live` frames from whatever it deduplicates and cursors on**, and only from that.

The lane also carries the agent's reasoning:

```json
{
  "type":         "REASONING_MESSAGE_CONTENT",
  "messageId":    "reasoning_live",
  "delta":        "the file is probably under src/",
  "chrono:live":  true
}
```

**`REASONING_START` and `REASONING_END` are not emitted, and their absence is deliberate.** This service sees a stream of thoughts and has no view of where one begins or ends, so a bracket sent from here would be a guess. Bracket on the run's own lifecycle events instead — they are exact — and clear reasoning when the run ends. Nothing of it survives, which is the point: reasoning is never committed, so a reload shows none of it and must not.

`THINKING_START`, `THINKING_END` and `THINKING_TEXT_MESSAGE_*` are AG-UI's deprecated spelling of this family. They are declared `false` in the capability document rather than omitted, so a client written against the older names is told they are not coming rather than left waiting on them.

`msg_live` is the in-flight message's identity, and it is deliberately not the committed message's. The committed one arrives under `msg_<seq>`, and a client **replaces** the in-flight message with it rather than appending — otherwise the turn is drawn twice, once as it was typed and once as it was recorded.

Declared as `chrono:liveFrames` in the capability document. Where it is false, or where a session's kernel offers no liveness, the durable lane is unaffected and the section below is the whole of what a client sees.

### Committed text arrives whole, not as tokens

On the durable lane, `TEXT_MESSAGE_CONTENT` carries the finished message rather than a stream of deltas. The token-level pieces are the live lane above, and they are deliberately not part of the durable log: numbering them would punch permanent holes in the sequence a resuming client cursors on, and replaying them would show somebody their conversation being typed a second time.

The consequence for a chat UI is worth stating plainly: **the log** gives you the message when the agent commits to it. If you want it while it is being written, read the live lane and expect nothing from it to survive a reload.

`messageId` is derived from the durable sequence, so reading the log twice yields the same identifier. A client deduplicating on it after a reconnect sees one message rather than two.

**`STATE_DELTA` is where `get_usage` lands.** Token counts arrive as RFC 6902 patches against `/usage`, so a chat UI can show spend without polling. A stretch of conversation the runtime had to drop arrives the same way, appended to `/gaps` — shown as a boundary rather than a stream that silently skips.

**A plane change lands there too**, appended to `/planeChanges`:

```json
{ "delta": [ { "op": "add", "path": "/planeChanges/-", "value": {
  "from": "terminal", "to": "conversation", "snapshotAt": "2026-08-18T09:14:22Z"
} } ] }
```

`snapshotAt` is the moment the conversation was captured, and it is where history stops being continuous: anything the agent did after it did not travel. Draw the boundary — a client that skips it silently leaves its reader to conclude the agent forgot something.

**The move ends the run.** A session being carried onto another runtime, whether onto a newer image or onto the other plane, goes `rotating`, and a `RUN_ERROR` is emitted. Nothing is being lost: the run is over because its runtime is being torn down, and a stream that simply stopped could not be told apart from an agent taking its time. Start a new run when the session is usable again — the same session, the same conversation, resumed.

### `RUN_ERROR` covers two different things, and says which

The protocol has one event for a run ending badly, and two things end one: the agent's work failed, and the session is being carried onto another runtime. **Do not draw them the same way.**

```json
{
  "type":            "RUN_ERROR",
  "message":         "this session's runtime can no longer be what it is",
  "code":            "error",
  "capability":      "conversation",
  "chrono:returns":  false
}
```

| Field | |
|---|---|
| `message` | what happened, in the kernel's own words. Always present. |
| `code` | the stop reason as this API spells it. Absent where the session did not stop for a reason it names. |
| `capability` | **the ability the session lacked**, present only when the failure was a refusal. Named rather than described: rule 4 of `API_Overview.md` asks errors to name what is missing, and a sentence that happens to contain the word does not satisfy that, because nothing downstream can read a sentence. Most failures lack nothing and carry no `capability` — a provider that was unreachable is not a session missing an ability. |
| `chrono:returns` | **`true` when the session is coming back.** This is a handover, not a fault: the run ended because the runtime behind it is being torn down, and a new run starts on the same session once it is usable. Draw it as a boundary, not as a failure. |

A client that ignores `chrono:returns` draws every rotation as a fault, and under §5 of `CLAUDE.md` a rotation is a routine consequence of a deployment. Treat its absence as `false`: that is the safer of the two to be wrong about, because a failure drawn as a handover tells somebody their session is returning when it is not.

### Tool kinds are the runtime's own word

`TOOL_CALL_START` carries `chrono:toolKind`, and `TOOL_CALL_END` carries `chrono:status`, both verbatim from the kernel. They are not mapped onto a fixed set: the vocabularies differ between kernels and grow without warning, and a session must not fail because one invented a category. Render what you recognise.

**`STATE_DELTA` is where `get_usage` lands.** Token counts and cost arrive as state patches rather than a separate call, so a chat UI can show spend without polling.

---

## 4. Approvals — the shape that surprises people

AG-UI **ends the run to ask, and starts a new run to resume.** An approval is not a mid-stream prompt.

```
1. Codex requests permission to run a command
2. chrono-bot ends the run:

   RUN_FINISHED {
     outcome: {
       type: "interrupt",
       interrupts: [{
         id: "apr_7Q2",
         reason: "chrono:command_execution",
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

`reason` is `confirmation` where the runtime offered no description of what it wants, and `chrono:<what the runtime called it>` where it did — `chrono:command_execution`, `chrono:file_write`, `chrono:network_egress` and whatever else a kernel names. The prefix is what the protocol reserves for a vendor's own values, and the word after it is the runtime's rather than one this service chose: the sets differ per kernel and grow.

`payload.decision` is `allow`, `deny`, or `abort`. Refusing one action and abandoning the turn are different intentions, and a client that meant the second and sent the first watches the agent try something else instead of stopping. **A decision or status this service does not recognise is read as a refusal** — the one direction this can safely fail in, because the alternative is granting on a word it did not understand.

Between step 2 and step 4 the session is `idle` with `stop_reason: requires_action`. **That state is durable.** A client that closes the tab and returns finds the interrupt still pending and answerable; it does not need to have held the stream.

`status: "cancelled"` on resume is a deny.

---

## 5. Interrupt and steer

- **Interrupt** — `POST /api/v1/sessions/{threadId}/interrupt`.

  **A dropped stream does not cancel anything.** This was specified the other way round — "abort the HTTP request and chrono-bot cancels the turn" — and that is wrong for two reasons that only became clear once the stream existed.

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
    "TOOL_CALL_START": true, "TOOL_CALL_ARGS": false, "TOOL_CALL_END": true,
    "TOOL_CALL_RESULT": false, "THINKING_START": false, "THINKING_END": false,
    "STATE_SNAPSHOT": false, "STATE_DELTA": true
  },
  "features": {
    "transport":      { "streaming": true, "resumable": true,
                        "websocket": false, "pushNotifications": false },
    "state":          { "snapshots": false, "deltas": true, "persistentState": true },
    "humanInTheLoop": { "supported": true, "approvals": true, "interrupts": true,
                        "approveWithEdits": false, "interventions": false },
    "execution":      { "codeExecution": true, "sandboxed": true },
    "custom":         { "chrono:terminal": false, "chrono:usage": true,
                        "chrono:changePlane": false, "chrono:planeChangeObserved": true }
  },
  "conformsTo": [
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/agui-core",
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/approvals"
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
- **Reconnect by reading, not by retrying the run.** A dropped stream is recovered with `GET /api/v1/agui/events?thread_id=<session>&starting_after=<seq>`, not by POSTing the run again — which would start a second turn. Read the cursor off the SSE `id`; it is not in the body.
- **`STATE_DELTA` patches, it does not replace.** Apply it to the last `STATE_SNAPSHOT`.

---

## 8. Stability

AG-UI is pre-1.0 and single-vendor, with no protocol version distinct from its package semver. chrono-bot therefore versions **its own** projection through the `conformsTo` URIs above, and never claims conformance to an AG-UI version — no such version exists to cite.

If AG-UI makes a breaking change, this interface's `conformsTo` URIs change and the core session model does not.
