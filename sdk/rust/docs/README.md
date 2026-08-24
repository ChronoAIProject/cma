# CMA Rust SDK

`cma-sdk` is the asynchronous backend client for CMA's REST interface. It is a
standalone Cargo project, uses Tokio and rustls, and has no dependency on CMA's
service crates.

## Install

The in-tree version remains `0.0.0`; the release pipeline stamps the published
crate from the repository tag. During repository development, use a path
dependency:

```toml
[dependencies]
cma-sdk = { path = "../chrono-bot/sdk/rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Authenticate

CMA accepts a bearer access credential. Supply it through a callback so a
long-running service can return the current value after rotation:

```rust
use cma_sdk::Client;

# fn client() -> Result<Client, Box<dyn std::error::Error>> {
let client = Client::builder("https://cma.example.com")?
    .credential_provider(|| std::env::var("CMA_ACCESS_TOKEN"))
    .build()?;
# Ok(client)
# }
```

The callback runs separately for every request attempt and stream reconnect.
Its result is marked sensitive before it enters the HTTP request. `Debug` for
the builder and client is implemented manually and never renders the callback
or its captured values.

## Create And Drive A Session

The complete runnable source is [`../examples/readme.rs`](../examples/readme.rs).
It creates a session, opens its typed event stream, sends input, and interrupts
the in-flight turn. The same file is compiled in the crate and copied into a
fresh scratch project by CI, proving that it uses only the published API.

Every operation accepts the client by shared reference. Dropping an operation
future or `EventStream` cancels only that work; it does not poison the client.
Unary requests have a per-request timeout. Transport failures reconnect with
bounded exponential backoff and jitter, while an explicit HTTP `Retry-After`
on `429` or `503` takes precedence.

## Errors

All operations return `cma_sdk::Error`:

- `CapabilityRefused { capability }` is permanent for this session and is
  never inferred from prose.
- `Api { status, code, message }` is a machine-readable service refusal.
- `Transport { message }` may succeed on a later attempt.
- `Protocol { message }` means the received boundary was malformed or drifted.

Known event types are fully typed. A future discriminator becomes
`EventKind::Unknown` carrying the complete JSON payload, so an older client can
advance its cursor without discarding or misrepresenting the event. A malformed
payload under a known discriminator is a `Protocol` error.

## Terminal Scope

Terminal resize and screen snapshot are included. The live bidirectional PTY
byte stream is deliberately absent from this version because it is a separate,
CLI-shaped public interface rather than a REST operation.
