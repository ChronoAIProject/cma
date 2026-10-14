# Agents

<!-- nav: 5 Agents -->

Use `agents().create_lifecycle` for Image Manager selections and inline Agent
configuration. Omit the image to use the deployment default. Persist the exact
request and key before submission so an uncertain result can be retried safely.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::agents::{AgentLifecycleCreateRequest, AgentLifecycleStatus};
# async fn provision(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let key = IdempotencyKey::new(std::env::var("CMA_CREATE_KEY")?)?;
let request = AgentLifecycleCreateRequest {
    workspace_id: None,
    sandbox_id: None,
    name: "Research".into(), image: None, repository: None, profile: None,
};
let receipt = client.agents().create_lifecycle(&request, &key).await?;
let operation = client.agents().read_lifecycle(&receipt.id).await?;
if operation.status == AgentLifecycleStatus::Succeeded {
    // Only now is operation.agent_id ready to open.
}
# Ok(())
# }
```

The receipt acknowledges durable admission. Poll `read_lifecycle` until global
success; a phase event's `ready` state is only one gate. `WaitingForConnection`
supplies missing services and an owner-bound connection target. After the owner
verifies those connections, repeat `create_lifecycle` with the original request
and key to resume the same operation. `Failed` is terminal and includes a safe
error when available.

Set `workspace_id: Some(existing_id)` to add an Agent to an owned, ready Workspace.
Omit `image` and `repository` in this mode. The Agent inherits the existing
Sandbox key scope; extra service scopes are refused and selecting a subset does
not reduce the inherited scope. This path requires permission to use the
Workspace and does not require permission to create a new Sandbox. Reusing the
same idempotency key with another Workspace is a conflict.

One Agent belongs to one allocated Worktree in an owned Workspace and Sandbox.
Its frozen Agent profile controls model, reasoning effort, permissions, and
skills. An Agent's conversation is shared by REST, AG-UI, and Responses.

```rust,no_run
use cma_sdk::{Client, EventOptions, IdempotencyKey};
use cma_sdk::agents::{AgentListOptions, AgentTextRequest};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let agents = client.agents().list(&AgentListOptions::default()).await?;
let id = std::env::var("CMA_AGENT_ID")?;
let key = IdempotencyKey::new(std::env::var("CMA_MESSAGE_KEY")?)?;
let input = AgentTextRequest { text: std::env::var("CMA_INPUT")? };
let accepted = client.agents().input(&id, &input, &key).await?;
let transcript = client.agents().transcript(&id).await?;
let usage = client.agents().usage(&id).await?;
let resources = client.agents().resources(&id).await?;
let history = client.agents().events(&id, EventOptions::default()).await?;
# let _ = (agents, accepted, transcript, usage, resources, history);
# Ok(())
# }
```

`create` takes an immutable Agent profile reference and allocates its Worktree.
`input` admits a user message; `steer` adds guidance to supported active work;
`interrupt` asks the Agent to stop its current turn. `answer_approval` targets
the exact server-issued approval identifier with `Allow`, `Deny`, or `Abort`.
`stop` and `delete` are distinct explicit lifecycle operations. Do not substitute
either for cancellation of local observation.

While a Sandbox checkpoint is active, input is refused before admission with
HTTP 409 `sandbox_checkpoint_in_progress`. No prompt is queued for later execution.
Refresh the Sandbox and submit again after `lifecycle.checkpoint` clears. This
also applies to AG-UI and Responses input; paused conversations still revive
automatically, and durable Trigger provisioning retains its own retry flow.

Durable transcripts and usage survive reconnects. Live reasoning and text
deltas are not durable transcript evidence. Resource counters are Sandbox-wide
and may be unavailable; `None` is not a zero measurement. The terminal helper
returns the named unsupported-operation refusal: this milestone has no PTY.

Render each transcript turn from its authoritative chronological `messages`
array. Every entry has a positive durable `sequence`, a closed `user` or
`assistant` `role`, exact `content`, and an RFC3339 `appended_at` timestamp. This
array preserves owner steering interleaved with Agent answers. `owner_message`
is only the initial owner-message summary; `agent_messages` contains only
assistant-message summaries. Concatenating those summaries loses the actual
conversation order and later owner guidance.
# Organization share management

`client.resource_access()` provides `effective`, `list_shares`, `read_share`,
`create_share`, `update_share` and `revoke_share`. Share kinds are `agent` and
`sandbox`; effective policy also accepts `workspace` and `worktree`.
`CreateShareRequest` contains `target_org_id` and `ShareMode`; an update also
requires `expected_revision`. Revoke takes the expected revision explicitly.
Every mutation requires `IdempotencyKey`. Reuse the same command/key to recover
the original receipt after an uncertain response.

The personal owner manages grants; recipients cannot reshare. Lists contain
active direct grants; individual reads retain revoked records. Unsafe narrow
interactive sharing requires current independent isolation or an explicit
compatible whole-Sandbox grant. Effective capabilities describe current policy,
not authorization for a later request.

`agents().read_accessible(id)` returns `AccessibleAgent::Personal(Box<Agent>)`
for a personal owner, or `AccessibleAgent::Shared(SharedAgent)` for another
authorized reader. Shared detail contains the Agent ID, status, optional title
and model, and timestamps, with `access: SharedAgentAccess::Shared`. It carries
no parent hierarchy, profile configuration, credentials or native conversation.
The existing `read(id)` method retains its full personal-detail type.

Shared transcript and event reads preserve messages and outcomes while removing
internal metadata and diagnostic details. `AgentEventKind::AccessRedacted`
retains the original sequence and timestamp for a withheld event; advance page
cursors normally. This never changes the durable log. Usage is Agent-specific;
Sandbox resource counters require a separate Sandbox authorization.

Reads obtain current membership and reject buffered results after a policy
change or expiration. Organization outages return the named
`organization_authority_unavailable` problem, and inaccessible Agents retain
`agent_not_found`. Live, file and interaction interfaces
remain under development.

`resource_access().organizations()` returns canonical organization choices and
current presentation roles. Use `agents().list_accessible(&options)` with
`AccessibleAgentListOptions { organization: Some(id), filters }` to list that
organization's owned and shared Agents. Omitting the organization preserves the
personal collection and the existing `list` method retains its personal type.
The new page contains `AccessibleAgentSummary::Personal` or `Shared` items.

```rust,no_run
use cma_sdk::{Client, agents::AccessibleAgentListOptions};
# async fn run(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let choices = client.resource_access().organizations().await?;
let organization = std::env::var("CMA_ORGANIZATION_ID")?;
let page = client.agents().list_accessible(&AccessibleAgentListOptions {
    organization: Some(organization),
    ..Default::default()
}).await?;
if let Some(first) = page.items.first() {
    let id = match first {
        cma_sdk::agents::AccessibleAgentSummary::Personal(agent) => &agent.id,
        cma_sdk::agents::AccessibleAgentSummary::Shared(agent) => &agent.id,
    };
    let source = client.agents().source_accessible(id).await?;
    assert_eq!(source.agent_id(), id);
}
# let _ = (choices, page);
# Ok(())
# }
```

Every page rechecks membership and grants. Overlap yields one Agent row, and
only authorized rows determine `next`. Parent filters require independent parent
access. A changed buffered page, including its hidden lookahead, returns
`resource_access_changed`; store outages return `resource_access_unavailable`.
The caller receives no partial page or unfiltered count.

`agents().source_accessible(id)` returns `AccessibleAgentSource::Personal` with
the existing creation provenance and published Trigger shortcuts, or `Shared`
with Agent ID and creation category only. Shared source has no private Launch
or Trigger IDs, profile index or shortcuts. Its access is rechecked after IO.
The existing `source(id)` method keeps its personal return type and rejects a
shared response, including manual/workspace categories.

`agents().provenance(id)` reads immutable creation facts from
`GET /api/v1/agents/{id}/provenance`. It returns `agent_id`, `created_at` and
`creation_origin`: `manual`, `trigger`, `channel_bridge` or `legacy_unknown`.
The channel category is reserved for bridge allocation; this increment implements
manual and Trigger admission plus conservative interpretation of retained records.

`details` is present only for the personal owner or a current manager of the
owning organization. It contains the captured `created_by`, original Trigger
references and optional automatic Workspace profile index. A missing creator is
unknown; the server never substitutes the resource owner. Trigger workers
record a Trigger actor and exact launch ID, with an unknown human/API launcher
unless that actor was captured explicitly. Shared recipients and
operators with read authority receive only the category and Agent timestamp.
Every response rechecks policy and the original authority lifetime before release.

Later Trigger reuse, profile changes, runtime revival and sharing preserve the
original creation facts. Automatic Workspace profile Agents retain their profile
index and currently report `legacy_unknown` when no supported creation category
was captured. This read does not write historical records. A separate bounded
background sweep captures conservatively inferred facts without rewriting
existing provenance or resource records. Missing creation evidence stays unknown;
inconsistent ancestry is refused. Active channel bindings and channel-use history
remain separate work.
The existing source method continues to provide its published Trigger shortcuts.

`agents().provenance_inventory(&options)` lists safe creation summaries from
`GET /api/v1/agents/provenance`. Personal ownership is the default; an explicit
organization requires current membership and includes organization-owned or
shared Agents. Rows have only Agent ID, optional title, status, creation/update
times and immutable category. Retained deleted and archived Agents are included
unless filtered by status. Parent filters require independent parent visibility.

Set `shared` to `true` for **Shared with me**: the deduplicated union of explicit
Agent and whole-Sandbox grants to the caller's current organizations, excluding
the caller's personally owned resources. Organization ownership alone does not
add a row to this scope. `shared=true` and `organization` are mutually exclusive.
Each page obtains fresh membership evidence; organization choices are only discovery
hints. An actual nonmembership result removes a stale choice, while an authority
outage refuses the page so an incomplete aggregate cannot appear complete.
Discovery is bounded to 128 organizations and the whole request to 15 seconds.
Personal inventory remains independent of organization discovery.

The default limit is 50 (maximum 200). Category filtering scans at most 201
candidates, so successful pages may be partial or empty while still having
`next`. **Follow every continuation until `next` is absent**, including after
an empty `items` array. Every consumed candidate and lookahead needs current
access. A changed grant, expired membership or storage failure refuses the whole
page. This method does not change the existing Agent list contract.

```rust,no_run
use cma_sdk::{Client, Result, agents::{AgentCreationCategory, AgentProvenanceInventoryOptions}};
async fn read_creation_inventory(client: &Client, organization: String) -> Result<()> {
    let mut options = AgentProvenanceInventoryOptions::default();
    options.scope.organization = Some(organization);
    options.creation_origin = Some(AgentCreationCategory::Manual);
    loop {
        let page = client.agents().provenance_inventory(&options).await?;
        for row in page.items { println!("{} {:?}", row.agent_id, row.creation_origin); }
        options.scope.filters.page.cursor = page.next;
        if options.scope.filters.page.cursor.is_none() { break; }
    }
    Ok(())
}
```

## Approved artifacts

Use `agents().list_artifacts(agent_id, &page)`, `read_artifact(agent_id, artifact_id)`
and `download_artifact(agent_id, artifact_id)` for explicit published content.
Downloads return opaque bytes; dropping the request future stops observation.
Every request requires current authorization. Narrow Agent shares can inspect
these artifacts without gaining raw repository or parent file access.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey, agents::PublishAgentArtifactRequest};
# async fn publish(client: &Client, agent_id: &str, key: &IdempotencyKey) -> cma_sdk::Result<()> {
let artifact = client.agents().publish_artifact(agent_id, &PublishAgentArtifactRequest {
    name: "Review.txt".into(),
    media_type: "text/plain".into(),
    content_base64: "cmV2aWV3".into(),
}, key).await?;
let bytes = client.agents().download_artifact(agent_id, &artifact.id).await?;
# Ok(())
# }
```

Only current sharing managers can publish. Explicitly approve content for the
Agent's current and future readers, retaining the same key/body across uncertain
results. No Agent input or runtime filesystem read occurs. Content is 1 byte
through 1 MiB, canonical standard base64, with a safe display name of at most
160 UTF-8 bytes. Plain text, Markdown, JSON, PDF, opaque binary, PNG, JPEG and
WebP are supported. At most 256 immutable artifacts are retained per Agent;
pages contain at most 50. Revoked authority prevents future reads but cannot
recall a file already downloaded.


## Channel usage

`agents().channel_usage(agent_id)` returns explicit channel facts independently
of immutable creation origin. `channel_usage` contains `active`, `has_history`,
and optional first/last accepted-command timestamps. Attachment alone leaves
those use timestamps absent; detachment preserves history. Manual and Trigger
Agents keep their creation category.

Only current resource managers receive `bindings`, including bridge/partition
references and generation lineage. Shared viewers receive the safe summary.
`channel_usage_history(agent_id, &page)` requires resource management authority.
The response includes the exact `agent_id`, ordered events, and an optional next
revision cursor. Pages default to 50 and accept limits from 1 through 200. Events
retain actor, timestamp, reason, changed binding identities, detach time, and
bounded before/after binding facts. The SDK checks identity, consecutive revisions,
and continuation integrity before returning a page.

```rust,no_run
use cma_sdk::{Client, PageOptions, Result};

async fn inspect_channel_use(client: &Client, agent_id: &str) -> Result<()> {
    let usage = client.agents().channel_usage(agent_id).await?;
    if usage.bindings.is_some() {
        let page = client.agents().channel_usage_history(agent_id, &PageOptions {
            limit: Some(25), cursor: None,
        }).await?;
        for event in page.items { println!("{} {:?}", event.revision, event.reason); }
    }
    Ok(())
}
```

These methods only observe facts. Binding history never grants authority, and
these routes cannot write or clear channel usage. Every request rechecks current
resource authority after membership or sharing changes.


`AgentProvenanceInventoryOptions` combines `channel_active` and `channel_history`
with `creation_origin` and existing personal, organization or shared scope.
`Some(false)` for active and `Some(true)` for history selects detached historical
bindings. The optional row `channel_usage` field supports older servers. Continue
with `next` after an empty filtered page: each request scans at most 200 authorized
candidates. Channel filters never relabel Manual or Trigger creation categories.

Resource access responses include an optional safe ownership summary, absent on
older servers. Personal ownership reports only whether the current caller owns
the resource; organization ownership supplies an ID for matching the caller's
current organization choices. Use the resource's current capabilities to display
access levels. Organization membership alone does not establish resource permissions.

## Live settings

`agents().models(agent_id)` returns the admitted model catalog with each model's
`id`, display `label`, and current runtime `available` flag. A retained runtime
may return the catalog with every choice unavailable.

`agents().settings(agent_id)` reads desired `model`, `reasoning_effort`,
`permission_mode` and `pending`. `update_settings(agent_id, &body, &key)` takes
`UpdateAgentSettingsRequest` with all three settings for an idle Agent and a
stable `IdempotencyKey`. Retry an uncertain HTTP reply with the exact body and
key. `pending: true` means the durable desired settings still await runtime
acknowledgement; read again until `pending` clears before treating them as applied.
The immutable creation profile is preserved.

## Additive skills

`agents().skills(agent_id)` returns profile and live skill items, their
`AgentSkillStatus` (`Pending`, `Installed`, or `Failed`), and `pending`,
`can_install`, and optional `unavailable_reason`. Items may include a display
name or failure detail. The server determines whether the Agent is idle and its
exact runtime supports installation.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey, Result, agents::InstallAgentSkillRequest};

async fn add_skill(client: &Client, agent_id: &str, key: &IdempotencyKey) -> Result<()> {
    let request = InstallAgentSkillRequest {
        id: "research".into(), version: "1.0".into(),
    };
    let admitted = client.agents().install_skill(agent_id, &request, key).await?;
    println!("Installation pending: {}", admitted.pending);
    let current = client.agents().skills(agent_id).await?;
    for skill in current.items { println!("{} {:?}", skill.id, skill.status); }
    Ok(())
}
```

Installation requires an exact Ornn version and a stable idempotency key. Retry
an uncertain reply with the same request and key. The `202` result records
admission; poll `skills()` within your application's deadline until `pending`
clears, then inspect each item's status before treating it as installed. A
failed installation is reported explicitly. Installation preserves the original
creation profile.

## Service access decisions

`agents().access_requests(agent_id)` reads `AgentAccessRequestList` for the
current owner. Each request carries its exact Agent identity, request ID,
service selection, reason and durable status. Approval requires an explicit
`ApproveAgentAccessRequest` and `IdempotencyKey`; denial uses
`deny_access_request(agent_id, request_id, &key)`. Services contain 1–32 ASCII
identifiers of 1–128 bytes using letters, digits, hyphens and underscores.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey, Result, agents::{AgentAccessRequest, ApproveAgentAccessRequest}};

async fn approve_services(
    client: &Client,
    agent_id: &str,
    request_id: &str,
    services: Vec<String>,
    key: &IdempotencyKey,
) -> Result<AgentAccessRequest> {
    client.agents().approve_access_request(
        agent_id, request_id, &ApproveAgentAccessRequest { services }, key,
    ).await
}
```

Use the same body and key after an uncertain reply: the durable service returns
current progress on replay. `AgentAccessStatus::Connecting` and `Applying` include
a connection target naming this access request for the owner's service connection
flow. Recheck that target if a connection expires while access is being applied;
reconnecting preserves the approved scope and the current request.
`Applying` awaits runtime verification; only `Succeeded` confirms applied access.
Read the list again after connecting services. `Denied` and `Failed` are terminal,
with an optional safe `error` detail. The SDK rejects crossed Agent/request
identities, inconsistent connection targets and duplicate list IDs. Listing never
approves services or resumes an Agent.
