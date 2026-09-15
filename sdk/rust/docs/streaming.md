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

Text is projected when messages commit. Provider tools and approvals remain in
the independently authorized AG-UI companion. Shared history redacts metadata
and closes on lost authority. Nested v2 HTTP errors retain their code,
`extensions["type"]`, `extensions["param"]`, request ID and retry delay; the
shared REST middleware envelope remains supported as well. See the
[API contract](../../../docs/API_Responses.md).

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

The server distinguishes `response_control_conflict` for a stale new intent,
`agent_command_key_conflict` for changed intent with an old key and
`capability_unsupported` for an unsupported runtime operation. Recover an
uncertain old admission before deciding whether a new intent is needed. Optional
discovery fields `controls` and `command_status` identify this extension while
older discovery remains readable. Standard Responses cancellation, cancellation
before turn dispatch and verified platform-sender controls are separate contracts.
