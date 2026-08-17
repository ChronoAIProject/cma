# AG-UI Interface

**Priority 1 — for frontend clients.**

AG-UI is how a chat frontend renders a chrono-bot session as a conversation: assistant prose, tool calls, reasoning, and approval prompts, streamed as they happen. It is the highest-priority interface because NyxID Chat and FKST Chat are the primary consumers.

See [API_Overview.md](./API_Overview.md) for the core operation set and session model.

---

## 1. What AG-UI is not for

AG-UI carries **semantic events**. It cannot carry a terminal byte stream — there is no honest mapping from `\x1b[2J\x1b[H` to a message event. A client that needs the literal terminal uses the [PTY interface](./API_PTY.md).

Consequently AG-UI projects **seven** of the nine core operations. `resize_terminal` and `screen_snapshot` are absent, and their absence is declared rather than discovered.

It projects **conversation sessions**. A session created in terminal mode carries no conversation capability, and a run against it is refused with the typed capability error naming `conversation` — before a stream opens, because a stream that opened and then said nothing would look like an agent thinking.

---

## 2. Endpoints

```http
POST /api/v1/agui/run          → text/event-stream
GET  /api/v1/agui/capabilities → application/json
```

A run is one request. The response is an SSE stream of AG-UI events that ends when the turn ends.

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
| `forwardedProps` | Session creation parameters, honoured only when the thread is new and ignored thereafter |
| `resume` | Present only when answering an interrupt — see §4 |

`forwardedProps` is ignored on an existing thread because `kernel` and `llmSource` are frozen at session creation. Changing them requires a new thread.

---

## 3. Events emitted

chrono-bot emits a **subset** of the AG-UI event vocabulary. The authoritative list is the `events` map in the capability document; this table is its documented form.

| Group | Emitted | Carries |
|---|---|---|
| Run lifecycle | `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR` | turn boundaries and outcome |
| Assistant text | `TEXT_MESSAGE_START`, `TEXT_MESSAGE_CONTENT`, `TEXT_MESSAGE_END` | what the agent said |
| Tool use | `TOOL_CALL_START`, `TOOL_CALL_END` | file edits, shell commands, MCP calls |
| State | `STATE_DELTA` | usage, and boundaries where conversation was lost |

Not emitted **today**: `TOOL_CALL_ARGS`, `TOOL_CALL_RESULT`, `THINKING_START`, `THINKING_END`, `STATE_SNAPSHOT`, and everything else in the enum. Each is declared `false` in the capability document rather than omitted from it, because omission reads as "this build has not heard of it" and a client sizing its handling needs the other claim. They arrive when the durable log carries what they would need: the core records that a tool call started and where it got to, not the arguments it was given or what it returned.

A client must tolerate unknown event types rather than failing on them, and must not assume an event absent from this table will never appear — the set grows additively.

### Text arrives whole, not as tokens

`TEXT_MESSAGE_CONTENT` carries the finished message rather than a stream of deltas. The token-level pieces exist — they travel as liveness on the interfaces that hold a live connection — but they are deliberately not part of the durable log: numbering them would punch permanent holes in the sequence a resuming client cursors on, and replaying them would show somebody their conversation being typed a second time.

The consequence for a chat UI is worth stating plainly: this stream gives you the message when the agent commits to it, not while it is being written.

`messageId` is derived from the durable sequence, so reading the log twice yields the same identifier. A client deduplicating on it after a reconnect sees one message rather than two.

**`STATE_DELTA` is where `get_usage` lands.** Token counts arrive as RFC 6902 patches against `/usage`, so a chat UI can show spend without polling. A stretch of conversation the runtime had to drop arrives the same way, appended to `/gaps` — shown as a boundary rather than a stream that silently skips.

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
    "custom":         { "chrono:terminal": false, "chrono:usage": true }
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
- **Reconnect through REST, not by retrying the run.** A dropped stream is recovered with `GET /api/v1/sessions/{id}/events?starting_after=<seq>`, not by POSTing the run again — which would start a second turn.
- **`STATE_DELTA` patches, it does not replace.** Apply it to the last `STATE_SNAPSHOT`.

---

## 8. Stability

AG-UI is pre-1.0 and single-vendor, with no protocol version distinct from its package semver. chrono-bot therefore versions **its own** projection through the `conformsTo` URIs above, and never claims conformance to an AG-UI version — no such version exists to cite.

If AG-UI makes a breaking change, this interface's `conformsTo` URIs change and the core session model does not.
