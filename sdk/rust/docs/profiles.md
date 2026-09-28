# Profiles

<!-- nav: 6 Profiles -->

Agent, Workspace, and Sandbox profiles have mutable owned drafts and immutable
published revisions. Select `agent_profiles()`, `workspace_profiles()`, or
`sandbox_profiles()` for a typed content boundary. Save uses draft-version CAS;
omitted name and description retain their current metadata.

`AgentProfileContent::isolated_agent` opts into separate process, file and
network boundaries. Missing JSON defaults to `false`, and serialization omits
that default to preserve existing wire requests. A true value survives profile
reads, updates and Bot Father setup reviews.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey, PageOptions};
use cma_sdk::profiles::{AgentProfileContent, MarketplaceListOptions, UpdateProfile};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let owned = client.agent_profiles().list(&PageOptions::default()).await?;
let id = std::env::var("CMA_AGENT_PROFILE_ID")?;
let current = client.agent_profiles().read(&id).await?;
let save = UpdateProfile { availability: None, draft_version: current.draft_version, name: None,
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

Profile authors can read deployment capacity bounds, safe service names, and
bounded skill catalog pages through `profile_authoring()`. Skill versions remain
literal `major.minor` strings in newest-first order; do not convert them to numbers.
Search uses at most 200 UTF-8 bytes, a limit from 1 through 50 (default 20), and
opaque nonempty continuation cursors of at most 2048 bytes, without controls.
Catalog descriptions allow up to 6144 UTF-8 bytes; profile draft text limits are unchanged.
An empty filtered page may still contain `next`; continue using that opaque cursor.
Only an absent `next` marks the end of the catalog.

```rust,no_run
use cma_sdk::{Client, profile_authoring::{ProfileAuthoringSkillSearch, ResolveRepositoryRequest}};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let authoring = client.profile_authoring();
let bounds = authoring.bounds().await?;
let services = authoring.services().await?;
let skills = authoring.search_skills(&ProfileAuthoringSkillSearch {
    q: Some("review".into()), limit: Some(20), cursor: None,
}).await?;
let repository = authoring.resolve_repository(&ResolveRepositoryRequest {
    url: std::env::var("CMA_REPOSITORY_URL")?, reference: None,
}).await?;
# let _ = (bounds, services, skills, repository);
# Ok(())
# }
```

All four operations require the profile-management capability. Repository
resolution is a read-only POST without an idempotency key: it neither clones a
repository nor admits work. Use the authoritative canonical URL and verified
lowercase commit identity; an omitted reference stays omitted so a Workspace
profile retains default-branch semantics. Repository and reference refusals are
typed API errors, not evidence that another URL or reference should be substituted.
