# Rust Streaming

<!-- nav: 50 Streaming -->

`Client::stream_events` returns a cancellation-safe `EventStream` over CMA's
durable REST log. Its optional cursor means strictly after that per-session
sequence. Persist a sequence only after committing the event's effect.

```rust,no_run
use cma_sdk::{Client, EventKind, SessionId};

async fn consume(client: &Client, session_id: &SessionId) -> cma_sdk::Result<u64> {
    let mut cursor = 0;
    let mut stream = client.stream_events(session_id, Some(cursor)).await?;

    while let Some(event) = stream.next().await {
        let event = event?;
        cursor = event.seq;
        match event.kind {
            EventKind::Unknown {
                event_type,
                payload,
            } => println!("future event {event_type}: {payload}"),
            EventKind::TurnCompleted => break,
            _ => {}
        }
    }
    Ok(cursor)
}
```

Reconnects send the last validated cursor in both REST's canonical
`starting_after` query and `Last-Event-ID`. One overlapping frame is deduplicated.
A frame with no positive SSE id, a mismatched payload sequence, another session
id, invalid UTF-8, malformed SSE, an oversized body, or a malformed known event
returns `Error::Protocol`.

## Reconnect and cancellation

- A transport disconnect reconnects after the client's bounded exponential delay.
- An SSE `retry` value takes precedence after a connected stream closes.
- Repeated empty connections stop at the configured retry-policy attempt count.
- The credential provider runs again for every reconnect.
- Dropping `EventStream` drops the active response, reconnect timer, and pending
  work without poisoning cloned clients.

An unrecognised event discriminator becomes `EventKind::Unknown` with the complete
JSON payload, so an older client can advance its cursor without pretending it
understood the event. Known discriminators remain strict. The full vocabulary and
replay rules are in [REST Events](../../../docs/API_REST.md#5-events).
