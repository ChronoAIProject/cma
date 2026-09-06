# Quickstart

<!-- nav: 2 Quickstart -->

Build one client for a deployment root and read its anonymous discovery
document:

```rust,no_run
use std::time::Duration;
use cma_sdk::Client;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder("https://cma.example.com/")?
    .request_timeout(Duration::from_secs(5))?
    .build()?;
let deployment = client.read_conformance().await?;
assert_eq!(deployment.service, "cma");
# Ok(())
# }
```

A trailing slash is normalised. Deployment URLs must use HTTP or HTTPS and may
not contain user information, a query, or a fragment.

Authenticated resource workflow:

```rust,no_run
use cma_sdk::{Client, EventOptions, IdempotencyKey};
use cma_sdk::agents::AgentTextRequest;
use cma_sdk::instances::{CreateSandboxRequest, SandboxListOptions};
use cma_sdk::profiles::SandboxProfileContent;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder(std::env::var("CMA_BASE_URL")?)?
    .bearer_token(std::env::var("CMA_BEARER_TOKEN")?)?
    .build()?;
let sandboxes = client.sandboxes().list(&SandboxListOptions::default()).await?;
for summary in sandboxes.items {
    println!("{}: {}", summary.sandbox.id, summary.sandbox.status.as_str());
}

let profile_id = std::env::var("CMA_SANDBOX_PROFILE_ID")?;
let published = client.marketplace::<SandboxProfileContent>().read(&profile_id).await?;
// Inspect the published commands and dependencies before granting trust.
let trust_confirmed = std::env::var("CMA_TRUST_CONFIRMED").as_deref() == Ok("1");
let create = CreateSandboxRequest {
    sandbox_profile: cma_sdk::profiles::ProfileReference {
        id: published.summary.id,
        revision: published.revision.summary.revision,
    },
    name: Some("Project".into()),
    trust_confirmed,
    commands_digest: published.trust.map(|trust| trust.commands_digest),
};
let create_key = IdempotencyKey::new(std::env::var("CMA_CREATE_KEY")?)?;
let sandbox = client.sandboxes().create(&create, &create_key).await?;
let status = client.sandboxes().read(&sandbox.id).await?;
println!("{}", status.summary.sandbox.status.as_str());

let agent_id = std::env::var("CMA_AGENT_ID")?;
let message = AgentTextRequest { text: "Review the current changes.".into() };
let message_key = IdempotencyKey::new(std::env::var("CMA_MESSAGE_KEY")?)?;
let accepted = client.agents().input(&agent_id, &message, &message_key).await?;
println!("queued: {}", accepted.queued);
let events = client.agents().events(&agent_id, EventOptions::default()).await?;
let transcript = client.agents().transcript(&agent_id).await?;
println!("{} durable events; {} turns", events.items.len(), transcript.turns.len());
# Ok(())
# }
```

Persist the chosen immutable profile revision, request body, and mutation key
together before sending a command. Do not reread a moving marketplace revision
and reuse the same key for different content after a timeout. A trust refusal
requires an explicit owner decision, not an automatic retry with confirmation.

The executable `examples/readme.rs` reads discovery and owned resources. It
only sends Agent input when `CMA_AGENT_ID`, `CMA_INPUT`, and
`CMA_IDEMPOTENCY_KEY` are explicitly supplied. `CMA_SDK_EXAMPLE_DRY_RUN=1`
compiles and constructs the exported calls without making network requests.
