# Streaming

<!-- nav: 7 Streaming -->

Streams are pull-based: `next().await` reads one complete event. Dropping a
reader cancels observation only. No reader reconnects or retries admission.
The configured request timeout covers headers and the full observation body;
choose a suitable finite bound for a followed stream and reconnect explicitly.

```rust,no_run
use cma_sdk::{Client, agui::ReplayOptions, launches::LaunchReplayOptions};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let agent = std::env::var("CMA_AGENT_ID")?;
let mut replay = client.agui().events(&agent, ReplayOptions { after: Some(0), follow: false }).await?;
while let Some(event) = replay.next().await {
    let event = event?;
    if let Some(cursor) = event.cursor { println!("complete durable record: {cursor}"); }
}
let launch = std::env::var("CMA_LAUNCH_ID")?;
let mut progress = client.launches().stream(&launch, LaunchReplayOptions::default()).await?;
while let Some(event) = progress.next().await { println!("{}", event?.seq); }
# Ok(())
# }
```

AG-UI can project one durable record into multiple frames. Only the final frame
advances its durable cursor. Standard SSE inherited IDs do not acknowledge
later live frames. Persist a cursor after application processing succeeds, not
merely after receiving bytes. Launch frames each carry one durable event and
must match both the requested Launch identity and the exact SSE sequence ID.

```rust,no_run
use cma_sdk::{Client, responses::CreateResponseRequest};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let request = CreateResponseRequest::new(std::env::var("CMA_AGENT_ID")?, std::env::var("CMA_INPUT")?);
let mut response = client.responses().create(&request).await?;
while let Some(event) = response.next().await { println!("{}", event?.kind.as_str()); }
# Ok(())
# }
```

Legacy Responses v1 sequence numbers are connection-local, starting at zero, not replay
cursors. Event names must agree with JSON types, and the stream must end with
`response.completed`, `response.failed`, or `response.incomplete`. On an early
disconnect, retrieve the known response or read durable Agent history before
deciding whether another message is appropriate. Streaming rejects oversized
frames at 1 MiB before the parser can accumulate unbounded data; HTTP problem
bodies are bounded at 8 MiB.

Agent replay accepts current organization/shared history access. Shared readers
receive approved events; private sign-in/configuration records become empty
`STATE_DELTA` patches with durable cursors. Acknowledge these cursors without
changing application state. Shared connections close on policy changes or at
the membership deadline (at most 60 seconds). Reconnect with the actual request
credential and the last fully acknowledged cursor; the new request checks
current membership again. Stream closure never submits another Agent input.
Personal-owner replay remains available during organization-provider outages.

## Retained keys for legacy POSTs

Use `agui().run_with_key(&request, key)` and
`responses().create_with_key(&request, key)` to retain an explicit invocation
key. Existing `run` and `create` calls remain valid unkeyed calls. The request
body and key are captured before credential/network work. Store them before
sending; the SDK never retries or changes a key after an uncertain reply.

A key accepts 1–256 bytes without control characters and must be representable
as an HTTP header. Separate unkeyed attempts remain separate commands. AG-UI
`runId` is correlation only, and ordinary input on a Working Agent queues a new
Message. Legacy Responses still requires the exact predecessor; absent/null
means the first turn, including on a retained keyed request.

The upgraded server reserves a common receipt before readiness and sends
`x-cma-command-id` / `x-cma-response-id` when known, including prebinding HTTP
errors. A binding observation timeout is uncertain work, never a synthetic
failed turn. Repeat the exact keyed POST explicitly to recover the same turn,
or use the public command status/history reads. Legacy wire IDs and sequence
rules are unchanged; receipt IDs do not replace them. Old servers ignored
these keys, so do not replay a potentially executed pre-upgrade request on the
assumption that a new key record can prove its history.

## Responses v2

`responses_v2()` uses durable command admission, opaque response IDs and
conversation on every turn without a predecessor. Persist a stable key before
sending to recover a lost first reply. `create_for_agent` is the per-Agent alias
with identical receipts and events. `read`, `read_conversation`, `list_items`
and `read_capabilities` provide authorized recovery and discovery.

```rust,no_run
use cma_sdk::{Client, responses_v2::{CreateResponseRequest, ResponseEventPayload}};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let agent = std::env::var("CMA_AGENT_ID")?;
let stable_key = std::env::var("CMA_DURABLE_REQUEST_KEY")?;
let request = CreateResponseRequest::new(agent, "Review the project");
let mut stream = client.responses_v2().create(&request, Some(&stable_key)).await?;
let receipt = stream.receipt().clone(); // Persist before reading frames.
let mut acknowledged = None;
while let Some(event) = stream.next().await {
    let event = event?;
    // Process and persist the event before advancing the durable cursor.
    if !matches!(event.payload, ResponseEventPayload::Error { .. }) {
        acknowledged = Some(event.sequence_number);
    }
}
let mut replay = client.responses_v2().replay(&receipt.response_id, acknowledged).await?;
while let Some(event) = replay.next().await { let _event = event?; }
# Ok(())
# }
```

Dropping the reader cancels observation only. No method reconnects or resubmits
input automatically. A connection `Error` payload has no explicit SSE ID and
must not advance the durable cursor. The next read returns a transport error;
retrieve or reconnect the retained response under current authorization. V2
checks exact per-frame IDs, contiguous sequences, lifecycle identity, output
indexes and assembled text. It rejects premature EOF and trailing events;
replay after a terminal cursor legitimately has no frames. The configured
finite timeout covers the entire observation; choose it for the expected turn.

Text is projected when messages commit. Match `OutputItem::Message` for text or
`OutputItem::Activity` for informational `mcp_call` activity. Activity carries an
empty arguments string, absent output/error bodies, and a display title; it never
asks the client to execute a tool. `McpCallInProgress`, `McpCallCompleted` and
`McpCallFailed` preserve stable indexes through interleaved text and replay.
Approval details remain in the independently authorized AG-UI companion. Shared history redacts metadata
and closes on lost authority. Nested v2 HTTP errors retain their code,
`extensions["type"]`, `extensions["param"]`, request ID and retry delay; the
shared REST middleware envelope remains supported as well. See the
[API contract](../../../docs/API_Responses.md).

Binding-key callers may supply an external conversation ID; the returned
conversation identifies the mapped Agent. The Agent alias and request conversation
may differ when the server resolves both to the same Agent. Known Agent IDs still
receive exact identity validation. An optional `event_context` retains the complete
schema-version-1 event snapshot atomically with admission. Preserve the original
body and idempotency key on retry. Changed context or another key for the same event
conflicts. The runtime receives its `event_ref`, while the complete object is read
with `responses_v2().read_event_context(binding_id, conversation_id, event_id)`.
The helper requires binding authentication and validates conversation/event identity.
The snapshot limit is 1 MiB; metadata and runtime input limits remain independent.

## Response-targeted controls

`responses_v2().observe_controls(agent_id, response_id)` returns the original
response command, optional bound turn, current Agent aggregate revision and
runtime support. A queued response has no bound target. Runtime support does not
grant control authority; CMA rechecks the caller and active approval at admission.

`submit_control` requires a retained string key and a strict `ControlRequest`.
Persist the Agent ID, response ID, key and serialized request before sending.
Recover uncertain admission with those identical values, including the old turn
and revision. The client sends once and never creates another key or retargets
a newer turn. Keys allow at most 256 bytes, no control characters, and no leading
or trailing whitespace that a client might normalize. Revisions and turns must
fit a positive signed 64-bit integer.

```rust,no_run
use cma_sdk::{Client, responses_v2::{ControlCommand, ControlRequest}};
# async fn run(client: Client, persist: impl FnOnce(&[u8]) -> std::io::Result<()>) -> Result<(), Box<dyn std::error::Error>> {
let agent = std::env::var("CMA_AGENT_ID")?;
let response = std::env::var("CMA_RESPONSE_ID")?;
let key = std::env::var("CMA_DURABLE_CONTROL_KEY")?;
let api = client.responses_v2();
let observed = api.observe_controls(&agent, &response).await?;
let target = observed.target.ok_or("response has no bound turn")?;
if !observed.target_active { return Err("response is no longer active".into()); }
let body = ControlRequest {
    turn_sequence: target.turn_sequence,
    expected_agent_revision: observed.agent_revision,
    command: ControlCommand::Cancel {},
};
// Persist all four values in the application's durable intent store first.
let saved_intent = serde_json::to_vec(&(&agent, &response, &key, &body))?;
persist(&saved_intent)?;
let accepted = api.submit_control(&agent, &response, &body, &key).await?;
let status = api.read_command(&accepted.control.receipt.command_id).await?;
if status.status.is_terminal() {
    // Completed control means delivery; this separate source status is the outcome.
    let source = api.observe_controls(&agent, &response).await?;
    println!("target outcome: {}", source.response.status.as_str());
}
# Ok(())
# }
```

`ControlCommand::Steer { text }` retains text bytes up to 256 KiB.
`ResolveApproval { approval_id, decision }` uses `ApprovalDecision::{Allow, Deny,
Abort}` and the current pending approval identity from the authorized companion.

`read_command` performs one GET under the client's configured request timeout.
Use application-owned bounded polling/backoff; dropping its future stops only
observation. Preserve delivery `RecoveryRequired` and `RuntimeLost`, command
`WaitingAction`, and typed `Error::Api` code/retry delay. A completed control
cannot establish cancellation: read the selected source response status for its
actual `Cancelled`, `Completed` or `Failed` outcome. Finishing normally before
cancellation takes effect is a valid race.

Inspect the ordinary API error's `code`: `response_control_not_admitted` proves
that the original scope/key has no receipt and its submitted revision is permanently
stale in the same Agent incarnation. Discard that pending intent, observe again,
and require a new explicit action before submitting another control.
`response_control_conflict` remains uncertain even on the first client call;
retain the original key and body for recovery. `agent_command_key_conflict` means
the key was reused for changed intent; `capability_unsupported` means the observed
runtime cannot perform the command. Optional
discovery fields `controls` and `command_status` identify this extension while
older discovery remains readable. Standard Responses cancellation and verified
platform-sender controls are separate contracts. Cancellation before turn binding
uses the dedicated methods below.

## Interrupt with new input

`interrupt_with_input` atomically admits cancellation of an observed active response
and a distinct new user input. Pass the original Agent and response IDs, a
`ResponseInterruptionRequest` containing `turn_sequence`, `expected_agent_revision`
and `text`, and one retained idempotency key. Use `observe_controls` to obtain the
current binding and revision before submission.

HTTP 202 returns `control` and `response` command views plus the original
`target` and `expected_agent_revision`. The control has no response ID; the new
input has a different response ID from the interrupted response. Its queue
priority places it before ordinary queued messages while preserving their
identities and relative order. Admission does not prove cancellation or new-turn
startup: observe the original response outcome and the new command separately.

After a lost response, retry the exact original IDs, body and key. The SDK never
retries or retargets automatically. Generic conflicts remain uncertain;
`response_interrupt_not_admitted` is the definitive proof that the original pair
was not admitted and its observed target became stale. Keep ordinary Stop on
`submit_control` with a cancel command; it admits no replacement message.

## Queued response cancellation

`observe_queued_cancellation(agent_id, response_id)` returns the source command,
its `command_revision`, and current `cancellable` observation. Eligible sources
are queued messages or unbound Channel messages waiting for action. CMA rechecks
fresh human/API operation authority and source state atomically; this observation
does not authorize a verified platform sender.

`cancel_queued_response` requires a `QueuedCancellationRequest` containing the
original source command identity and source command revision, plus a retained
string key. The revision belongs to the command, not the Agent. Persist the
route, body and key before sending; the client never retries mutations,
generates another key, or replaces a source selection after a lost reply.

```rust,no_run
use cma_sdk::{Client, responses_v2::QueuedCancellationRequest};
# async fn run(client: Client, persist: impl FnOnce(&[u8]) -> std::io::Result<()>) -> Result<(), Box<dyn std::error::Error>> {
let agent = std::env::var("CMA_AGENT_ID")?;
let response = std::env::var("CMA_RESPONSE_ID")?;
let key = std::env::var("CMA_DURABLE_CANCEL_KEY")?;
let api = client.responses_v2();
let observed = api.observe_queued_cancellation(&agent, &response).await?;
if !observed.cancellable { return Err("response is no longer cancellable before dispatch".into()); }
let body = QueuedCancellationRequest {
    source_command_id: observed.response.receipt.command_id,
    expected_command_revision: observed.command_revision,
};
persist(&serde_json::to_vec(&(&agent, &response, &key, &body))?)?;
let accepted = api.cancel_queued_response(&agent, &response, &body, &key).await?;
println!("source outcome: {}", accepted.response.status.as_str());
# Ok(())
# }
```

HTTP 200 returns a completed `cancellation` receipt without a response ID, turn
or runtime delivery, the original immutable `target`, and the source command's
current `response` view. The receipt establishes committed cancellation before
dispatch. Exact recovery preserves the cancellation receipt and target while the
current source view may change. Standard Responses event variants stay unchanged.

Recover an uncertain reply by explicitly resubmitting the identical route, body
and key. `queued_response_cancellation_conflict` remains uncertain even on the
first call. `queued_response_cancellation_not_admitted` proves a committed absence
of the original receipt and permanent staleness of the original source revision;
discard that pending intent, observe again and require a new explicit action
before forming another intent. `agent_command_key_conflict` identifies changed
input under a retained key. The ordinary API error retains each exact code.
Optional discovery `queued_cancellation` describes the extension; older discovery
remains readable.

## Rich Agent replay and transport eligibility

`client.agent_events_v2()` exposes authenticated `read_transport_eligibility`,
bounded `snapshot`, and read-only `events`. The flag selects transport only for
new tabs; it is not command authority. A missing endpoint, invalid schema, or
unavailable eligibility result cannot select a new v2 tab. Existing v2 tabs
retain their original accepted/uncertain input when the flag turns off; never
replay that input through v1.

Snapshots project safe `agent_events_v2::AgentEventKind` values with complete
tool status/title, messages, approvals, usage, and original turn timestamps. A
null kind is a withheld row that still advances history. Opaque `AgentCursor`
coordinates bind one Agent incarnation; numeric `sequence` orders chronology
independently of standard Responses sequence numbers. `AgentPageCursor` is a
distinct snapshot continuation. Every page has at most 200 rows and 4 MiB through
its original `through_cursor`; apply every page before tailing that head.

```rust,no_run
use cma_sdk::{Client, Result};
use cma_sdk::agent_events_v2::{AgentCursor, AgentFrame, TailOptions};

async fn follow_agent(
    client: &Client,
    agent_id: &str,
    saved_cursor: AgentCursor,
    apply_and_save: impl Fn(AgentFrame) -> Result<()>,
) -> Result<()> {
    let mut stream = client.agent_events_v2().events(agent_id, &TailOptions {
        after_cursor: Some(saved_cursor), ..Default::default()
    }).await?;
    while let Some(result) = stream.consume_next(|frame| {
        let applied = apply_and_save(frame);
        async { applied }
    }).await {
        result?;
        // Advances only after apply_and_save returned success for a durable frame.
        let acknowledged = stream.acknowledged_cursor();
        let _ = acknowledged;
    }
    Ok(())
}
```

The application callback must handle `AgentFrame::Event` by applying the complete
durable event and atomically persisting normalized state plus its cursor under
the account/session and Agent scope before returning success. Handle `Live` and
`LiveGap` only in ephemeral display state; never store reasoning summaries.
`Error` reports safe observation failure and then closes the stream. A failed
consumer closes observation while retaining the previous acknowledged cursor.
Make durable application safe to repeat after a crash between processing and
the storage commit. Live frames and gaps never advance a durable cursor;
reconnection loss has no invented frame count and missed reasoning cannot replay.

For bootstrap, call `snapshot` with `SnapshotOptions` and follow each exact
`next_page_cursor` while preserving the page limit. Apply every row, including
null-kind rows. Only after the final page has no continuation should the application
atomically save normalized state with `through_cursor` and start `events` there.
If bootstrap cannot commit that state, restart at its previous durable checkpoint.

Reconnect explicitly with the retained cursor. `last_event_id` is the standard
header alternative; supplied query/header coordinates must agree.
`follow: Some(false)` reads through a captured head and ends. Dropping a stream
cancels observation only. No read resubmits input, retargets control, switches
protocol, or automatically reconnects. A normal close may reflect expired
authority; reopen with current authorization and application backoff.

Typed `Error::Api` codes distinguish malformed/future cursors, changed
incarnation, inaccessible resources, unavailable history, and unavailable reads.
Strict parsing refuses unrecognized/private fields, nonconsecutive chronology,
invalid source bindings, and inconsistent pages. Changed incarnation requires
rebuilding history for the new resource; verified response binding is still an
observation rather than control authority.


### Edit pending input in place

The public queue lists every editable human/API message in its original admission
order, including messages submitted from another tab. The server admits at most
64 pending commands. HTTP pages contain at most four entries and a nullable
`next_after` admission-order cursor, keeping worst-case JSON escaping within the
SDK's unchanged 8 MiB response limit. The SDK reader follows these pages and
returns the full bounded list; it rejects repeated cursors, duplicate commands or
more than 64 accumulated entries. Pages observe current state independently;
concurrent edits to earlier pages appear on the next refresh. Refresh after admission,
edit, cancellation, observed conversation changes, and returning to the page.
The ledger does not emit conversation events before dispatch; clients displaying
an active queue should also refresh periodically to observe other tabs.

An edit preserves the source command, response and queue position. It replaces
the complete text, including packed attachment text, only before runtime dispatch.
Persist the route, exact source revision, text and a new idempotency key before
sending. A lost reply must be recovered with identical bytes and the same key.
`queued_input_not_admitted` proves the original revision is stale; refresh the
queue before creating another intent. The edit receipt returns its immutable
replacement text and resulting revision, alongside the source's current status.
Removal continues to use queued cancellation with a fresh observation.

```rust
use cma_sdk::Client;

async fn edit_first_queued_input(
    client: &Client, agent_id: &str, retained_edit_key: &str,
) -> cma_sdk::Result<()> {
    let queue = client.responses_v2().list_queued_inputs(agent_id).await?;
    if let Some(item) = queue.data.first().filter(|item| item.editable) {
        let input = cma_sdk::responses_v2::QueuedInputRequest {
            source_command_id: item.response.receipt.command_id.clone(),
            expected_command_revision: item.command_revision,
            text: "Updated message, including any attachment text".into(),
        };
        client.responses_v2().edit_queued_input(
            agent_id, item.response.receipt.response_id.as_deref().unwrap(),
            &input, retained_edit_key,
        ).await?;
    }
    Ok(())
}
```
