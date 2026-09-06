use cma_sdk::agents::{AgentListOptions, AgentTextRequest};
use cma_sdk::instances::SandboxListOptions;
use cma_sdk::profiles::{AgentProfileContent, MarketplaceListOptions};
use cma_sdk::{Client, EventOptions, IdempotencyKey, PageOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dry_run = std::env::var_os("CMA_SDK_EXAMPLE_DRY_RUN").is_some();
    let base_url = if dry_run {
        "https://example.invalid".to_string()
    } else {
        std::env::var("CMA_BASE_URL")?
    };
    let builder = Client::builder(base_url)?;
    let client = if dry_run {
        builder.build()?
    } else {
        builder
            .bearer_token(std::env::var("CMA_BEARER_TOKEN")?)?
            .build()?
    };
    if dry_run {
        drop(client.read_conformance());
        drop(client.agent_profiles().list(&PageOptions::default()));
        drop(client.workspace_profiles().list(&PageOptions::default()));
        drop(client.sandbox_profiles().list(&PageOptions::default()));
        drop(
            client
                .marketplace::<AgentProfileContent>()
                .list(&MarketplaceListOptions::default()),
        );
        drop(client.sandboxes().list(&SandboxListOptions::default()));
        drop(client.workspaces().list("sandbox-1"));
        drop(client.worktrees().list("workspace-1"));
        drop(client.agents().list(&AgentListOptions::default()));
        drop(client.agents().events("agent-1", EventOptions::default()));
        drop(client.agents().transcript("agent-1"));
        drop(client.agents().usage("agent-1"));
        drop(client.agents().resources("agent-1"));
        let key = IdempotencyKey::new("example-message-1")?;
        let message = AgentTextRequest {
            text: "Review changes.".into(),
        };
        drop(client.agents().input("agent-1", &message, &key));
        return Ok(());
    }

    let deployment = client.read_conformance().await?;
    println!("{}: {}", deployment.service, deployment.status);
    for summary in client
        .sandboxes()
        .list(&SandboxListOptions::default())
        .await?
        .items
    {
        println!(
            "{}: {}",
            summary.sandbox.id,
            summary.sandbox.status.as_str()
        );
    }
    if let Ok(agent_id) = std::env::var("CMA_AGENT_ID") {
        let agent = client.agents().read(&agent_id).await?;
        println!("{}: {}", agent.id, agent.status.as_str());
        if let Ok(text) = std::env::var("CMA_INPUT") {
            let key = IdempotencyKey::new(std::env::var("CMA_IDEMPOTENCY_KEY")?)?;
            let accepted = client
                .agents()
                .input(&agent_id, &AgentTextRequest { text }, &key)
                .await?;
            println!("queued: {}", accepted.queued);
        }
        let transcript = client.agents().transcript(&agent_id).await?;
        println!("{} durable turns", transcript.turns.len());
    }
    Ok(())
}
