# CMA Rust SDK

<!-- nav: 1 Overview -->

The repository-only Rust SDK provides typed async access to CMA discovery,
Agent/Workspace/Sandbox profiles, the profile marketplace, Sandboxes,
Workspaces, Worktrees, and Agents. It is a standalone client package with no
service-crate dependencies and is not published to a package registry.

Use it from a checkout with a Cargo path dependency:

```toml
[dependencies]
cma-sdk = { path = "../cma/sdk/rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust,no_run
use cma_sdk::Client;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::builder("https://cma.example.com")?.build()?;
let deployment = client.read_conformance().await?;
println!("{}: {}", deployment.service, deployment.status);
# Ok(())
# }
```

Resource requests accept a caller-supplied bearer credential. Every mutation
requires an explicit `IdempotencyKey`; reuse the same key and unchanged request
when retrying one logical command. The client never retries automatically and
never follows redirects.

Durable REST event polling, transcript, usage, and resource reads are supported.
This package does not implement live AG-UI, Responses, PTY, MCP, or A2A streams.
REST acceptance does not prove that provisioning, input execution, or a
lifecycle operation has finished. Poll the returned resource and durable events
for the relevant status.
