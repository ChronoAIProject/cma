# Rust Operations

<!-- nav: 60 Operations -->

Every method borrows `&Client`, so cloned clients and independent operations share
configuration rather than mutable session state. Unary requests have the builder's
request timeout; the event-stream timeout covers each connection handshake.

## Input and turn control

```rust,no_run
use cma_sdk::{Client, SessionId, SessionInput};

async fn drive(client: &Client, session_id: &SessionId) -> cma_sdk::Result<()> {
    client
        .send_input(session_id, SessionInput::text("Run the tests"))
        .await?;
    client
        .steer(session_id, "Run the integration tests first")
        .await?;
    client.interrupt(session_id).await?;
    Ok(())
}
```

`SessionInput::text` uses the line-submission default. Use
`text_without_newline` for terminal text that must not submit a line, and `bytes`
for raw terminal input. Check `Session::conforms_to` before exposing an operation.
Input is deliberately refused while a session rotates.

## Approvals

Resolve an `EventKind::ApprovalRequested` identifier with one typed decision:

```rust,no_run
use cma_sdk::{ApprovalDecision, Client, SessionId};

async fn deny(client: &Client, session_id: &SessionId) -> cma_sdk::Result<()> {
    client
        .resolve_approval(session_id, "approval_example", ApprovalDecision::Deny)
        .await
}
```

`Allow` permits the action, `Deny` refuses it while letting the turn continue, and
`Abort` refuses it and asks the runtime to abandon the turn.

## Terminal REST and usage

```rust,no_run
use cma_sdk::{Client, SessionId, TerminalGeometry};

async fn inspect_terminal(client: &Client, session_id: &SessionId) -> cma_sdk::Result<()> {
    client
        .resize_terminal(session_id, TerminalGeometry::new(120, 32)?)
        .await?;
    let screen = client.screen_snapshot(session_id).await?;
    let usage = client.get_usage(session_id).await?;

    println!("{} rows, {} input tokens", screen.rows, usage.input_tokens);
    Ok(())
}
```

Resize and screen snapshot are REST projections. This crate does not expose the
live bidirectional PTY WebSocket, transcript download, pricing, resource readings,
or environment administration. The live terminal contract is
[API_PTY.md](../../../docs/API_PTY.md).
