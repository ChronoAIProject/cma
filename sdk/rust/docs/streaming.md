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

`EventKind::RuntimeHandoverStarted` is a unit event written immediately before
every marker-aware handover enters `Rotating`. It permanently enables
marker-based usage accounting for the session, so rollback and readoption status
changes do not split the outgoing runtime's cumulative counter. History without
this discriminator retains its legacy `Rotating` boundaries.

`EventKind::RuntimeIncarnationStarted` is a unit event for compatibility stores
without durable fencing generations. It is committed atomically with the record
that names a newly provisioned runtime, and every such marker separates
cumulative token counters. A provider attempt that never gets a durable handle
writes none; an idempotently retried commit writes the unit once.

`EventKind::RuntimeStarted` carries a positive durable `generation`. It marks a
committed runtime-authority change, not a candidate start; duplicate or older
generations do not begin another incarnation. Usage readers use strictly newer
markers to separate cumulative token counters that restart on replacement. A
runtime-start marker also enables exact accounting when no handover marker
preceded it.
