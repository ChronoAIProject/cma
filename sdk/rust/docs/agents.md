# Agents

<!-- nav: 5 Agents -->

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
was captured. This read does not persist a historical backfill. Active channel
bindings, channel-use history, inventory filters and UI badges are separate work.
The existing source method continues to provide its published Trigger shortcuts.
