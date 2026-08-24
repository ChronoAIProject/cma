# Rust Quickstart

<!-- nav: 20 Quickstart -->

This Tokio program creates a session, opens the durable event stream at the
session's last known sequence, submits one owner message, and waits for the turn
to complete.

```rust,no_run
use cma_sdk::{Client, CreateSessionRequest, EventKind, SessionInput, SessionStatus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder("https://cma.example.com")?
        .credential_provider(|| std::env::var("CMA_ACCESS_TOKEN"))
        .build()?;

    let session = client
        .create_session(CreateSessionRequest {
            environment_profile: std::env::var("CMA_ENVIRONMENT_PROFILE").ok(),
            ..CreateSessionRequest::default()
        })
        .await?;
    if !session
        .conforms_to
        .iter()
        .any(|class| class.ends_with("/conf/conversation"))
    {
        return Err("the selected session profile is not conversation-shaped".into());
    }
    let mut events = client
        .stream_events(&session.id, Some(session.last_seq))
        .await?;

    let mut input_sent = session.status == SessionStatus::Ready;
    if input_sent {
        client
            .send_input(
                &session.id,
                SessionInput::text("Summarise this repository"),
            )
            .await?;
    }

    while let Some(event) = events.next().await {
        let event = event?;
        if !input_sent {
            match event.kind {
                EventKind::StatusChanged {
                    status: SessionStatus::Ready,
                    ..
                } => {
                    client
                        .send_input(
                            &session.id,
                            SessionInput::text("Summarise this repository"),
                        )
                        .await?;
                    input_sent = true;
                }
                EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    ..
                } => return Err("the session terminated before it became ready".into()),
                _ => {}
            }
        } else if matches!(event.kind, EventKind::TurnCompleted) {
            break;
        }
    }
    Ok(())
}
```

The complete runnable package example is
[`examples/readme.rs`](../examples/readme.rs). CI copies that file into a fresh
consumer crate, installs `cma-sdk` by path as an external dependency, and runs a
dry branch before any network request.

Creation can return while the session is still provisioning. Use the durable
events for progress, and call `read_session` when a current full projection is
needed. `CMA_ENVIRONMENT_PROFILE` is optional when the deployment's unqualified
default is conversation-shaped; otherwise it must name a conversation profile.
Continue with [Authentication](authentication.md) before putting the client in a
long-running service.
