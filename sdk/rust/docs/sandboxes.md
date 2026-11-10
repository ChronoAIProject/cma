# Sandboxes

<!-- nav: 4 Sandboxes -->

Sandbox lists include retained history by default. Set `exclude_retired: true` in the
list options to omit only `retired` records before pagination; keep the same filter
when following a cursor. Paused, failing, and retiring Sandboxes remain visible.
The option applies to personal, organization, and incoming shared inventories,
combines with the exact status filter, and does not delete any retained record.
The public query parameter is `exclude_retired=true`; false or omission preserves
the historical listing. Combining it with `status=retired` returns an empty page.

Agent Sandbox persists a Sandbox on its attached persistent volume. CMA does not
create, schedule, expose, download, or delete Sandbox snapshot archives. Pause,
revive, and retirement delegate runtime and volume lifecycle to Agent Sandbox;
the CMA lifecycle view reports only the current transition eligibility and safe
operation progress.

A Trigger can cancel a pause before agent parking and teardown are admitted.
The `pause` operation reports `ResumeWriters` until the exact hold is released.
If teardown may already have started, the Launch waits for pause completion and
revives automatically. Navigation away does not cancel an admitted Launch.

A Sandbox pins its profile, provider, resource limits, service scope, commands,
and environment at admission. It contains Workspaces, each with allocated
Worktrees and Agents. A successful create response records work; it does not
assert that the runtime or any nested Agent is ready.

`sandboxes().grant_service_access(id, &GrantSandboxServiceAccessRequest, &key)`
records explicit owner authorization for additional NyxID services while fully
paused. Original profile pins and creation scope remain unchanged. The optional
`nyxid_services` field reports effective scope, including these grants. Revival
obtains a new scoped credential before delivering it; the owner's connected
accounts must authorize every service. New grants while Live return HTTP 409
`sandbox_service_access_requires_pause`. Repeating existing grants is a no-op,
including after revival. This operation only adds services; it does not revoke
existing access.

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
`checkpoint_in_progress` or `work_in_progress`; these are transient admission
signals while Agent Sandbox persists the attached volume. The optional operation
records a pause, revival or image rotation phase without worker or writer
credentials. Missing lifecycle data from an older server means eligibility is
unavailable, not that status alone authorizes an action. A race with new work can
return HTTP 409 `sandbox_lifecycle_busy`; refresh before retrying. Existing
command replay semantics are unchanged.

The owner's list, read and create responses carry an optional `Sandbox.color`:
the colour every Agent in that Sandbox wears, as lowercase `#rrggbb`. The server
assigns it and keeps it distinct from the owner's other live Sandboxes. It is
`None` until assigned, on shared-grant reads and on older servers; any other
spelling fails deserialization.

## Managed runtime updates

Owner detail includes optional `Sandbox.runtime_update` information for managed
native runtimes. `latest_version` is the server's formally released immutable
image reference; publishing an image alone does not change it. The server checks
for updates daily at 02:00 Asia/Singapore, and an owner-initiated revival from
Paused admits the current managed version. Custom-image Sandboxes are excluded.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::instances::{SandboxRuntimeUpdateStatus, UpdateSandboxRuntimeRequest};
# async fn update(client: &Client, id: &str, key: &IdempotencyKey) -> cma_sdk::Result<()> {
let detail = client.sandboxes().read(id).await?;
if let Some(update) = detail.summary.sandbox.runtime_update {
    if update.can_update {
        let target_version = if update.state == SandboxRuntimeUpdateStatus::Failed {
            update.target_version.unwrap_or(update.latest_version)
        } else {
            update.latest_version
        };
        let request = UpdateSandboxRuntimeRequest { target_version };
        let accepted = client.sandboxes().update_runtime(id, &request, key).await?;
        println!("{:?}", accepted.runtime_update);
    }
}
# Ok(())
# }
```

HTTP 202 records the request; it does not mean replacement has completed. Keep
the exact target and idempotency key across uncertain responses, and follow
`runtime_update.state` until completion or a reported failure. Retry a failed
request with its retained `target_version`, even if a later release changes
`latest_version`.

Manual and scheduled updates wait for admitted work, active turns and queued
messages to settle before replacing the runtime. They retain the Sandbox,
Workspaces, Agents, conversations and original persistent home. An initially
paused Sandbox returns to Paused unless new owner work requests a revival.

## Shared hierarchy

Use `sandboxes().read_accessible`, `workspaces().read_accessible` and
`worktrees().read_accessible` for a typed personal/shared union. Its `Personal`
variant retains the full detail, while `Shared` contains safe metadata without
owner/profile configuration, repository paths, credentials or runtime internals.
The old `read` methods keep their full personal types and refuse shared output.
`workspaces().list_accessible(sandbox_id)` and
`worktrees().list_accessible(workspace_id)` return corresponding child unions.

```rust,no_run
use cma_sdk::{Client, instances::AccessibleSandboxDetail};
# async fn inspect(client: &Client, id: &str) -> cma_sdk::Result<()> {
match client.sandboxes().read_accessible(id).await? {
    AccessibleSandboxDetail::Personal(detail) => println!("{}", detail.summary.sandbox.id),
    AccessibleSandboxDetail::Shared(detail) => println!("{}: {} Agents", detail.id, detail.agent_count),
}
# Ok(())
# }
```

A narrow Agent grant cannot reveal any parent or its counts. Whole-Sandbox or
organization access is checked for each hierarchy request, including current
policy after the read. Default Sandbox inventory excludes explicit organization
ownership before pagination; use `list_accessible` with an organization for
explicit discovery.

## Organization and Shared with me inventory

```rust,no_run
use cma_sdk::{Client, instances::AccessibleSandboxListOptions};
# async fn inventory(client: &Client, organization: String) -> cma_sdk::Result<()> {
let page = client.sandboxes().list_accessible(&AccessibleSandboxListOptions {
    organization: Some(organization),
    ..Default::default()
}).await?;
for sandbox in page.items {
    println!("{}", sandbox.id());
}
# Ok(())
# }
```

Organization-owned resources and whole-Sandbox grants enter this explicit scope
under current canonical membership. Narrow Agent grants never reveal a parent
or its counts. `AccessibleSandboxSummary::Personal` retains the full personal
summary; `Shared` contains safe metadata/counts. Leave both scope selectors at
their defaults for personal inventory. The old `list` method keeps its personal type. Every page
revalidates current policy and the hidden lookahead after count IO; changed or
expired authority yields a typed error without partial rows or stale cursors.

Set `shared: true` in `AccessibleSandboxListOptions` to discover incoming
whole-Sandbox grants across all current organizations. The server deduplicates
Sandboxes and excludes your own personal resources before pagination. Explicit
organization ownership alone does not enter this scope. Individually shared
Agents remain discoverable through the shared provenance inventory without
exposing parents. Combining `shared: true` with `organization: Some(...)` fails
local validation. Unavailable organization authority refuses incomplete discovery;
every page revalidates all original membership evidence and grants.

## Shared lifecycle commands

`sandboxes().submit_command(id, &request, &key)` admits an explicit lifecycle
intent with HTTP 202. `read_command(command_id)` recovers its safe receipt with
HTTP 200. Shared detail `revision` remains optional for older servers; require a
current positive revision before submission.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::instances::{SandboxCommandKind, SandboxCommandRequest};
# async fn request_pause(client: &Client, id: &str, revision: u64, key: &IdempotencyKey) -> cma_sdk::Result<()> {
let request = SandboxCommandRequest {
    command: SandboxCommandKind::Pause,
    expected_revision: revision,
};
let receipt = client.sandboxes().submit_command(id, &request, key).await?;
let current = client.sandboxes().read_command(&receipt.command_id).await?;
# let _ = current;
# Ok(())
# }
```

Retain the exact request and key through an uncertain response. Replay uses the
original revision even after the Sandbox changes. Receipt states are `Queued`,
`Applied`, and `Rejected`. Applied records a durable transition or an already
scheduled/satisfied intent; current Sandbox status reports runtime completion.
Rejected state carries a typed reason. The SDK validates request/receipt identity,
state revision and resulting Sandbox revision before reporting success.

Pause/revive require current Sandbox management. Retirement requires owner/admin
sharing management plus the personal owner's deletion capability when applicable;
share recipients cannot retire the owner's Sandbox. Execution rechecks the original
actor and current resource authority. Receipt reads require that actor's current
Sandbox visibility. Queue deadlines never renew. Public receipts include no owner
configuration, credential reference or internal audit.

## Measured resource reports

Use `client.usage_metrics()` for owner reports and `client.admin_usage_metrics()`
for administrator reports. `create(&usage_metrics::Query::default())` accepts
200 ready or 202 pending. Poll `report(&report_id)` with a bounded application
policy respecting Retry-After; the SDK never polls indefinitely. Once ready,
`sandboxes`, `sandbox` and `series` share that immutable report ID. Administrator
`owners` searches the complete frozen cohort by owner-subject prefix.

The report exposes the requested range, finalized `effective_range`, cutoff,
independent metric timestamps/coverage and optional admin infrastructure footer.
CPU is cores, RAM is working-set bytes, and disk is persistent-home filesystem
bytes. Period core-hours/GiB-hours are observational estimates; allocated billing
and charges remain separate. Agent filtering still returns shared Sandbox usage.
Null and stale/unmounted values are not zero. Report expiry returns 410; queue
or source pressure returns 429/503 with Retry-After. Reauthorization applies to
every cached request.

`usage_metrics::Quantity::Number(f64)` represents ordinary values and
`Quantity::Decimal(String)` preserves large integer quantities. Integer fields
use lossless u64 parsing of either safe JSON integers or canonical decimal
strings. Choose an explicit presentation conversion; never silently convert
large byte/second strings into floating-point invoice evidence.

### Owner allocation rankings

`client.admin_usage_metrics().owner_allocations(report_id, &query).await` reads a bounded owner allocation page from an existing administrator
report. Results cover the full selected cohort, ordered by allocation descending,
with `total_count` and a signed `next` cursor. Supply that cursor with the same
report and resource to continue. It does not create another report.

Display `label`, and show `disambiguator` only as secondary duplicate-name context.
`subject` is an opaque key and must not appear in chart text or tooltips. Respect
partial `complete` flags, retain loaded/total counts on failures, and handle
429/503 retries and 410 report expiry explicitly.

## Retained runtime assessment

`client.sandboxes().inspect_runtime_recovery(id).await` reads the owner-only
retained-runtime assessment. Its fixed disposition is diagnostic and
`admission_enabled` is false. GET performs no recovery or lifecycle action.
