# Sandboxes

<!-- nav: 4 Sandboxes -->

A Sandbox pins its profile, provider, resource limits, service scope, commands,
and environment at admission. It contains Workspaces, each with allocated
Worktrees and Agents. A successful create response records work; it does not
assert that the runtime or any nested Agent is ready.

```rust,no_run
use cma_sdk::{Client, EventOptions, IdempotencyKey};
use cma_sdk::instances::{SandboxListOptions, CreateWorkspaceRequest};
use cma_sdk::profiles::ProfileReference;
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let page = client.sandboxes().list(&SandboxListOptions::default()).await?;
for row in page.items {
    let detail = client.sandboxes().read(&row.sandbox.id).await?;
    println!("{}: {} workspaces", detail.summary.sandbox.id, detail.workspaces.len());
}
let sandbox = std::env::var("CMA_SANDBOX_ID")?;
let workspace_profile = ProfileReference { id: std::env::var("CMA_WORKSPACE_PROFILE_ID")?, revision: 1 };
let key = IdempotencyKey::new(std::env::var("CMA_WORKSPACE_KEY")?)?;
let request = CreateWorkspaceRequest { workspace_profile, trust_confirmed: false, commands_digest: None };
let workspace = client.workspaces().create(&sandbox, &request, &key).await?;
let tree = client.workspaces().read(&workspace.id).await?;
let events = client.sandboxes().events(&sandbox, EventOptions::default()).await?;
# let _ = (tree, events);
# Ok(())
# }
```

Set trust confirmation only after the owner inspects the returned command
disclosure and exact digest. `false` is omitted on the wire, never silently
upgraded to consent. Retain the immutable reference and idempotency key across
ambiguous transport failures.

`workspaces().list`, `worktrees().list`, and `worktrees().read` navigate the
owned hierarchy. Worktree allocation is Agent-owned; there is no standalone
Worktree mutation helper. Sandbox `pause`, `revive`, and `delete`, and Workspace
`delete`, return recorded lifecycle state. Poll events and resources until the
requested outcome is observed. Deletion and retirement are explicit owner
actions, not cleanup implied by a dropped stream or failed Launch.

The optional `Sandbox.lifecycle` contains authoritative `pause` and `revive`
eligibility for beginning a new transition. An active Sandbox can be busy with
`checkpoint_in_progress` or `work_in_progress`. Recorded checkpoint progress
contains the `capture` or `resume_writers` phase, request timestamp and epoch;
the optional operation records a pause, revival or image rotation phase without
worker or writer credentials. Missing lifecycle data from an older server means
eligibility is unavailable, not that status alone authorizes an action. A race
with new work can return HTTP 409 `sandbox_lifecycle_busy`; refresh before
retrying. Existing command replay semantics are unchanged.
