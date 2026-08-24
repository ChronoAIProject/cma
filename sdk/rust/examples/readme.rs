use cma_sdk::{Client, CreateSessionRequest, EventKind, SessionInput};

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
        .create_session(CreateSessionRequest::default())
        .await?;
    let mut events = client
        .stream_events(&session.id, Some(session.last_seq))
        .await?;
    client
        .send_input(&session.id, SessionInput::text("Summarise this repository"))
        .await?;

    while let Some(event) = events.next().await {
        if matches!(event?.kind, EventKind::TurnStarted) {
            client.interrupt(&session.id).await?;
            break;
        }
    }
    Ok(())
}
