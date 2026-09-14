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

Responses sequence numbers are connection-local, starting at zero, not replay
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
