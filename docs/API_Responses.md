# Responses Interface

**Priority 2 - for chat clients and OpenAI-compatible SDKs.**

CMA serves a stateful, streaming subset of the OpenAI Responses API at
`/api/v1`. It is a Layer 1 projection of the same durable Agents used by
the other interfaces: creating a conversation creates an Agent context, and
creating a response sends one turn through the shared Layer 2 operation path.
The conversation identifier is the Agent id. There is no separate Chat model
or `surface` selector.

The subset is deliberately closed. A field or operation outside it is refused
with a named `422 application/problem+json` response rather than accepted and
ignored silently.

See [API_Overview.md](./API_Overview.md) for shared lifecycle semantics and
[API_REST.md](./API_REST.md) for authentication, request limits, errors, and
deployment-wide rate limiting.

---

## 1. Supported resources

All routes accept either a CMA browser cookie or a bearer credential under the
REST authentication contract.

| Method and route | Supported behavior |
|---|---|
| `POST /api/v1/conversations` | Create a Worktree-backed Agent using defaults or owned context overrides |
| `GET /api/v1/conversations/{id}` | Read one caller-owned Agent as a conversation |
| `GET /api/v1/conversations/{id}/items` | Read newest-first durable history |
| `POST /api/v1/responses` | Admit one turn and stream it as SSE |
| `GET /api/v1/responses/{response_id}` | Reconstruct one durable response |
| `DELETE /api/v1/responses/{response_id}` | Explicit authenticated `422` refusal |
| `POST /api/v1/responses/{response_id}/cancel` | Explicit authenticated `422` refusal |

Unknown identifiers, conversations owned by another subject,
and response identifiers that do not name a retained turn
all return the same `404`. This concealment prevents resource existence from
crossing an ownership boundary.

Conversation identifiers are CMA Agent identifiers. Conversation metadata
includes `agent_profile`, `workspace` and `sandbox`, plus server-owned `title`
when one exists. Response identifiers are stable projections of the Agent and
the durable turn sequence:

```text
resp_<agent-id>_<turn-sequence>
```

User message item identifiers use their durable event sequence. Assistant
message identifiers use the response turn sequence. Conversation items are
returned newest first with an OpenAI list envelope. `limit` accepts `1` through
`100`; `after` must be an item identifier from the same conversation.

The standard conversation and item behavior used as the compatibility point is
documented in the official OpenAI references for
[creating conversations](https://developers.openai.com/api/reference/resources/conversations/methods/create.md),
[retrieving conversations](https://developers.openai.com/api/reference/resources/conversations/methods/retrieve.md),
and [listing conversation items](https://developers.openai.com/api/reference/resources/conversations/subresources/items/methods/list.md).

---

## 2. Response field policy

`POST /api/v1/responses` accepts a JSON object. The final user content must be
either a non-empty `input` string or one array item containing a single user
message with text content.

| Field | CMA v1 policy |
|---|---|
| `conversation` | Required and honored; must name a caller-owned Agent |
| `input` | Required and honored; exactly one user text message |
| `stream` | Required as literal `true` |
| `metadata` | Honored; at most 16 string pairs, with bounded keys and values |
| `previous_response_id` | Honored as the linear-thread predecessor fence |
| `model` | Optional string, accepted but ignored; response uses the Agent Profile's pinned model |
| `store` | Omitted or `true`; `false` is refused because chat history is durable |
| `instructions` | Refused by name |
| `reasoning` | Refused by name |
| `tools` and `tool_choice` | Refused by name; caller-defined tools are unavailable |
| `background` | Refused by name |
| `stream_options`, `starting_after` and `include` | Refused by name; stream resume is unavailable |
| Any unknown field | Refused by its supplied field name |

Conversation creation accepts `{}` or metadata containing only owned
`agent_profile`, `workspace` and `sandbox` identifiers. Each supplied value is
nonempty, at most 128 bytes and contains no control characters. Unknown keys,
including caller-supplied `title`, are refused. Omitted context is resolved
through platform defaults. Specifying both Workspace and Sandbox requires
matching owned lineage; it does not move the Workspace.

```json
{
  "metadata": {
    "agent_profile": "agp_example",
    "workspace": "wks_example",
    "sandbox": "sbx_example"
  }
}
```

The `metadata` on a response POST is separate: it accepts at most 16 string
pairs, with keys at most 64 characters and values at most 512 characters.

Threads are linear. On the first turn, `previous_response_id` must be absent or
null. On every later turn it must equal the latest response identifier for that
conversation. Omitting it after a turn exists, naming a stale response, naming
another conversation, or naming malformed data returns `409
previous_response_id_conflict` before the prompt reaches the runtime. Clients
should retrieve current history and retry from its latest response rather than
creating a fork.

Stateless response creation, non-streaming creation, background execution,
stream resumption, caller tools, response deletion, and response cancellation
are not part of this subset. The authenticated DELETE and cancel routes exist
so those absences are discoverable and typed. The request and retrieval shapes
track the official OpenAI references for
[creating responses](https://developers.openai.com/api/reference/resources/responses/methods/create.md)
and [retrieving responses](https://developers.openai.com/api/reference/resources/responses/methods/retrieve.md).

---

## 3. Stream contract

Successful response creation returns `200 text/event-stream`. Every data frame
has a `type` and a zero-based `sequence_number`; sequence numbers are contiguous
for that connection. The SSE `event` field repeats `type`.

| Event | Meaning |
|---|---|
| `response.created` | The turn was admitted; the projected response is initially in progress |
| `response.in_progress` | Initial lifecycle projection is open |
| `response.reasoning_summary_part.added` | A live reasoning summary part began |
| `response.reasoning_summary_text.delta` | Live-only reasoning summary text |
| `response.reasoning_summary_text.done` | The live reasoning text ended |
| `response.reasoning_summary_part.done` | The live reasoning summary part ended |
| `response.output_item.added` | The assistant message began |
| `response.content_part.added` | Its output-text part began |
| `response.output_text.delta` | Assistant text delta |
| `response.output_text.done` | Durable assistant text is complete |
| `response.content_part.done` | The completed output-text part |
| `response.output_item.done` | The completed assistant message |
| `response.cma_chat.tool_activity` | Namespaced CMA tool activity extension |
| `response.completed` | Durable turn completion with usage |
| `response.failed` | Terminal failure with `response.error` |
| `response.incomplete` | Terminal incomplete response with details |

`response.cma_chat.tool_activity` carries an `item` whose type is
`cma_chat.tool_activity`. It includes the tool call id, kind, target and status;
`duration_ms` is currently null. Tool updates without a retained kind use
`"unknown"`. Argument and result bodies are not part of this projection.
The same extension item can appear in durable conversation history.

Reasoning has a narrower lifetime. Live `ThoughtDelta` frames project into the
reasoning-summary events above, but reasoning text is never written into
conversation items and is never reconstructed by response retrieval. A client
that was disconnected cannot recover missed reasoning. Assistant text, tool
activity retained by policy, status, metadata, and usage are reconstructed
from the durable event log.

The standard event vocabulary used here follows the official
[Responses streaming events reference](https://developers.openai.com/api/reference/resources/responses/streaming-events.md)
and [streaming guide](https://developers.openai.com/api/docs/guides/streaming-responses.md).
The two `cma_chat.*` types are CMA extensions, not OpenAI event or item types.

---

## 4. Disconnects and recovery

The response turn is owned by CMA after admission. Dropping the HTTP response
body, navigating away, aborting a client-side reader, or losing the connection
does not cancel the core turn. There is no response-stream resume endpoint in
v1. Reconnect by reading the response resource when its identifier is known and
list conversation items to recover durable output. If a terminal response is
not yet available, retry the read under the ordinary REST retry policy.

Posting a response to an Agent in a paused Sandbox also requests revival. CMA
opens the response only after checkpoint restoration and prompt admission, and
then streams the same event sequence as an already-live host. A restored host
uses a new runtime generation while the conversation and response identifiers
remain tied to durable Agent history.

Admission is bounded by ten minutes. If provisioning or restoration does not
make the conversation reachable by that deadline, the stream contains one
`response.failed` event with code `provisioning_timeout`. Because no durable
`TurnStarted` sequence exists yet, CMA uses the provisional identifier
`resp_<agent-id>_0` for this event. That provisional `_0` identifier is
explicitly CMA behavior, is not a durable retrievable response identifier, and
must not be attributed to the standard OpenAI API.

Response retrieval never reads a response database. It identifies the durable
turn, bounds it at the next `TurnStarted`, and folds the retained event log into
output, status, metadata, configured model, and usage. An incomplete historical
turn followed by a newer turn is treated as corrupt history and is not exposed
as a still-live response.

The stateful design corresponds to the official OpenAI
[conversation state guide](https://developers.openai.com/api/docs/guides/conversation-state.md),
with CMA's stricter required-conversation and linear-predecessor rules declared
above.

---

## 5. Declaration

`GET /api/v1/conformance` includes:

```text
https://spec.chrono-ai.fun/cma/1.0/conf/responses-chat
```

The Responses interface projects Agent-context creation, input, retained output
and usage. It does not grant additional capabilities to an Agent, change its
profile, create a terminal or add a Trigger mutation route.

---

## 6. Official SDK conformance

Local compatibility was verified on 2026-09-07 with `openai-node` 7.10.0 on
Node 26.8.1 and `openai-python` 3.8.0 on Python 3.13.11. Both clients passed
against CMA's loopback-only `official_responses_sdk_server` test composition.
The checked-in runners are
[`openai_node.mjs`](../api/crates/cma/tests/official_responses/openai_node.mjs)
and [`openai_python.py`](../api/crates/cma/tests/official_responses/openai_python.py).

Each client created and retrieved an Agent conversation in its default owned
context, read 18 raw streaming frames, consumed 16 aggregate-helper frames,
and paginated three durable conversation items. Assertions covered contiguous
event sequences, live reasoning, CMA tool-activity extensions, reconstructed
assistant output and metadata, and the typed linear-predecessor conflict.
Reasoning was absent from reconstructed history and completed output.

The fixture uses real CMA public handlers, Core context/operation services and
durable update recording with an in-memory store and a scripted runtime port.
It does not contact OpenAI or NyxID, invoke a real agent kernel, or prove
production provisioning or storage. Its owned Sandbox and Workspace already
exist; fresh-Sandbox/default-context admission and context override validation
are covered separately by backend tests and the shared public contract corpus.

Run the ignored server test explicitly, never as part of an unfiltered ignored
test run. It prints `CMA_SDK_BASE_URL`; both runners require that exact loopback
URL. The Node runner also requires `CMA_NODE_SDK_ENTRY` pointing at the installed
SDK's `index.mjs`; the Python runner uses its environment's installed `openai`
package. No real API key is needed. The fixture accepts only its static local
test credential and exposes an authenticated `POST /__sdk/shutdown` for orderly
shutdown, with a ten-minute lifetime bound as a fallback.
