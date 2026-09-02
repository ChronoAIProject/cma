# Rust Reference

<!-- nav: 80 Reference -->

The API reference is generated from the crate's rustdoc and types; signatures are
not copied into this guide. Generate and open it locally with:

```sh
cargo doc --no-deps --open
```

The crate is repository-only and carries `publish = false`, so it has no crates.io
release or docs.rs page. The integration files in this directory appear under
`cma_sdk::guides`, and their code fences run during `cargo test --doc`.

The public export list is in [`src/lib.rs`](../src/lib.rs). `Client` and
`ClientBuilder` own transport configuration, `ResponseMetadata` is the
pre-decoding response-correlation projection, `Conformance` is anonymous
deployment discovery, `EventStream` owns the reconnecting durable stream,
`ResponsesStream` owns one non-reconnecting response turn, `types` are wire
projections, and `Error` is the non-exhaustive failure boundary. The crate
intentionally does not expose its internal HTTP, decoding, or stream-parser
modules.

## Responses chat

```rust,no_run
use cma_sdk::{Client, CreateResponseRequest, ResponsesEvent};

async fn chat(client: &Client) -> cma_sdk::Result<()> {
    let conversation = client.create_conversation().await?;
    let request = CreateResponseRequest::text(
        conversation.id.clone(),
        "Summarise this repository",
    )?;
    let mut stream = client.create_response(request).await?;
    let mut terminal = None;

    while let Some(event) = stream.next().await {
        match event? {
            ResponsesEvent::Completed { response, .. }
            | ResponsesEvent::Failed { response, .. }
            | ResponsesEvent::Incomplete { response, .. } => terminal = Some(response),
            _ => {}
        }
    }

    if let Some(response) = terminal {
        let durable = client.read_response(&response.id).await?;
        println!("{}: {:?}", durable.id, durable.status);
    }
    Ok(())
}
```

Response creation makes one POST and never reconnects or retries an ambiguous
handshake. Dropping `ResponsesStream` closes the local body without calling the
unsupported server cancellation route; recover retained output with
`read_response` and `list_conversation_items`. The exact supported field and
event policy is in the [Responses contract](../../../docs/API_Responses.md).

Unknown event discriminators are retained in `EventKind::Unknown`; malformed known
events and unknown session enum values fail as `Error::Protocol`. See
[Sessions](sessions.md) and [Streaming](streaming.md) for those compatibility rules.
