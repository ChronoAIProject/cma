# Profiles

<!-- nav: 6 Profiles -->

Agent, Workspace, and Sandbox profiles have mutable owned drafts and immutable
published revisions. Select `agent_profiles()`, `workspace_profiles()`, or
`sandbox_profiles()` for a typed content boundary. Save uses draft-version CAS;
omitted name and description retain their current metadata.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey, PageOptions};
use cma_sdk::profiles::{AgentProfileContent, MarketplaceListOptions, UpdateProfile};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let owned = client.agent_profiles().list(&PageOptions::default()).await?;
let id = std::env::var("CMA_AGENT_PROFILE_ID")?;
let current = client.agent_profiles().read(&id).await?;
let save = UpdateProfile { draft_version: current.draft_version, name: None,
    description: None, content: current.draft };
let key = IdempotencyKey::new(std::env::var("CMA_PROFILE_SAVE_KEY")?)?;
let updated = client.agent_profiles().save(&id, &save, &key).await?;
let public = client.marketplace::<AgentProfileContent>()
    .list(&MarketplaceListOptions::default()).await?;
# let _ = (owned, updated, public);
# Ok(())
# }
```

`create` starts an owned draft. `publish` requires its exact draft version;
`list_revisions` and `read_revision` inspect immutable history. `set_visibility`
and `retire` do not rewrite pinned instance configuration. Marketplace `read`
returns public immutable content and any required trust disclosure. `fork`
requires an explicit source revision rather than a moving latest revision.

Public profile values pass the backend public-value scanner. A refusal identifies
the environment variable by name, never echoes its value. Move actual secrets
into supported scoped service credentials; do not work around publication
validation by renaming or encoding the value. Retirement can be refused while
published Triggers reference a profile, with structured Trigger identities in
the problem extensions.
