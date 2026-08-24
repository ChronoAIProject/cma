use cma_sdk::{Client, CreateSessionRequest, EventKind, SessionInput, SessionStatus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder("https://cma.example.com")?
        .credential_provider(|| std::env::var("CMA_ACCESS_TOKEN"))
        .build()?;

    // CI runs this branch from a fresh consumer crate. Real integrations leave
    // the variable unset and execute the operations below.
    if std::env::var_os("CMA_SDK_EXAMPLE_DRY_RUN").is_some() {
        println!("{client:?}");
        return Ok(());
    }

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
            .send_input(&session.id, SessionInput::text("Summarise this repository"))
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
                        .send_input(&session.id, SessionInput::text("Summarise this repository"))
                        .await?;
                    input_sent = true;
                }
                EventKind::StatusChanged {
                    status: SessionStatus::Terminated,
                    ..
                } => return Err("the session terminated before it became ready".into()),
                _ => {}
            }
        } else if matches!(event.kind, EventKind::TurnStarted) {
            client.interrupt(&session.id).await?;
            break;
        }
    }
    Ok(())
}
