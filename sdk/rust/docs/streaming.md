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
