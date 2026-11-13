# Agents

<!-- nav: 5 Agents -->

`client.agents().suggestions(agent_id, cma_sdk::answer_context::AnswerContext {
turn_sequence: 3, answer_sequence: 5 }).await` reads a saved contextual result.
This GET never generates or sends input. A `ready` reading contains a stable
`SuggestedReplySet`; pending, empty, unavailable, failed and expired states do
not block ordinary conversation. Preserve the returned scope for an unchanged
suggestion's normal conditional send. Observations retain measured tokens and
latency, with provider-reported cost when available and `None` when unknown.

Use `agents().create_lifecycle` for Image Manager selections and inline Agent
configuration. Omit the image to use the deployment default. Persist the exact
request and key before submission so an uncertain result can be retried safely.

```rust,no_run
use cma_sdk::{Client, IdempotencyKey};
use cma_sdk::agents::AgentLifecycleCreateRequest;
# async fn provision(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let key = IdempotencyKey::new(std::env::var("CMA_CREATE_KEY")?)?;
let request = AgentLifecycleCreateRequest {
    desktop: None,
    initial_message: Some("Compare three tools and cite your sources.".into()),
    workspace_id: None,
    sandbox_id: None,
    name: "Research".into(), machine_included: false, image: None, repository: None, profile: None,
};
let receipt = client.agents().create_lifecycle(&request, &key).await?;
let _operation = client.agents().read_lifecycle(&receipt.id).await?;
# Ok(())
# }
```

The receipt acknowledges durable admission. Read `read_lifecycle` and verify
its exact `agent_id` and owned parent hierarchy before opening the preparing
conversation. Operation success means preparation, not task completion;
a phase event's `ready` state is only one gate. `WaitingForConnection`
supplies missing services and an owner-bound connection target. After the owner
verifies those connections, repeat `create_lifecycle` with the original request
and key to resume the same operation. `Failed` is terminal and includes a safe
error when available.

`initial_message` preserves exact text, including whitespace and newlines, in
the original idempotent request. It must contain non-whitespace text and fit
262144 UTF-8 bytes. The server queues it durably as `purpose: task` after
environment preparation, replacing the readiness prompt. Omission retains
legacy `setup` behavior. With `repository`, a new Sandbox prepares the pinned
source checkout before delivering the task; omit existing Workspace and Sandbox
IDs in this mode. Discover support in the served OpenAPI request schema and its
`x-cma-task-repository` extension. Do not send a separate first message after
creation. Read Agent detail/transcript for delivery, recovery and results.

For a new desktop Sandbox, set `machine_included: true` and omit `image`.
`agents().read(id).await?.desktop` reports its desktop capability. Call
`agents().desktop(id).await?` to discover the same-origin `websocket_url`; your host
WebSocket implementation carries binary RFB frames on that path using owner
authentication. Browser connections require the exact CMA Origin, and one viewer
may connect to a Sandbox at a time. Unavailable desktops return named API errors.

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

`agents().recover_worktree(id, &key)` retries failed initial worktree preparation
only for an Agent that has never become ready. The owner or a CMA administrator
can request recovery. It preserves the Agent identity and persistent volume,
retains the failed worktree and its recovery evidence, and allocates a fresh
worktree for the same Agent. HTTP 202 acknowledges admission; use `read` to
observe readiness. Reuse the exact key after an uncertain response. Already-ready
or otherwise ineligible Agents return HTTP 409 `agent_worktree_recovery_conflict`.

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

## Skill installation and upgrades

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
clears, then confirm the requested version and inspect `error`. The same operation
upgrades an installed ID on a capable idle runtime. During an upgrade its single
row retains the old effective version and `Installed` status. A failed upgrade
keeps that version and names the failed target in `error`. The original creation
profile remains immutable. After a definitive busy or unsupported-runtime refusal,
make a new attempt with a new key; the old key continues replaying its refusal.

## Service access decisions

Direct NyxID owner access tokens can approve requests and replace service scope
without a browser login. CMA verifies the owner, current management permissions,
finite token expiry and access to each selected service and its backing nodes.
Runtime/Agent keys, relay tokens, service accounts and delegated actors cannot
approve. The existing personal-resource ownership policy still applies.

CMA stores the exact API token in encrypted credential custody and records only
its authority reference with the decision. Deferred execution, credential
issuance, renewal and revocation recover that same authority after restart and
revalidate it; they never substitute a browser login. API tokens are not refreshed
by CMA. Expired or revoked authority stops further credential operations with
`owner_service_authority_required`; authorize further work with a current owner
token. On the original Agent, submit a new idempotency key with both `services`
and `expected_services` equal to the current scope. Once the prior request has
settled or failed, this records fresh consent for that exact scope, including
when credential rotation or revocation is still pending. Pending rotation
continues at its existing scope generation; settled access performs an ordinary
rotation with a fresh backing-node plan. Both preserve the prior audit, runtime
and conversation. A changed scope remains a separate update after rotation settles. Replaying an old key returns its original
result and never substitutes new authority.

HTTP 403 `agent_access_owner_authority_required` requests a direct owner access
token. HTTP 409 `agent_access_service_connection_required` means the owner must
connect the service or authorize its backing resources through NyxID, then retry.
The existing connection workflow remains available to browser owners. Missing
scope-plan support or invalid provider evidence returns a temporary refusal;
CMA never silently issues a service-only key.

Managed keys include the exact backing nodes required by the approved services. CMA obtains a complete NyxID v1 scope plan and sends its digest as a creation precondition; broad node/service grants remain disabled. Missing, incomplete or changed plans refuse issuance. Renewal resolves the current configured routes again. If a service moves to another node, the old key fails closed until a new scoped key is issued; hot replacement revokes the old key before reporting success. Route-only Bot keys keep empty service and node grants.

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

An explicit retry after a failed access request is a new pending consent attempt.
Its optional `retry_of` names the preserved failed request. The Agent tool returns
new attempt parameters after failure; old-key replay continues to return the old
outcome. New attempts require fresh owner approval and never inherit an earlier
approval or runtime update.

To add or revoke services directly, call `agents().update_service_access(agent_id, &body, &key)` with `UpdateAgentServiceAccessRequest { services, expected_services }`.
`services` is the exact desired shared Sandbox scope; `expected_services` is the
last observed effective scope. Both must include `chrono-llm-public` and
`ornn-api`. Save is explicit owner consent. The service returns HTTP 202 and the
durable access request, and rejects stale selections with HTTP 409 before
changing access. Preserve the same body and key after an uncertain reply.
Prepare any external service connections before final approval. An active
Sandbox receives its replacement credential without restarting Agents;
`succeeded` confirms runtime acknowledgement, old-key revocation and callback
delivery to the originating Agent. An already paused Sandbox waits for resume.

## Runtime observations

Owned Agent detail and hierarchy entries optionally expose `runtime_metadata`
with the provider's canonical `working_directory` and session `description`.
The `runtime_metadata_changed` event updates the same snapshot. These fields
do not rewrite the immutable Workspace/Worktree paths or infer changes from
shell commands. Shared readers do not receive private directory metadata.

`description_source` optionally identifies `provider_title` or `prompt_preview`;
absence means unknown. `description_is_setup_preview: true` is positive evidence
that this provider preview came from the exact recorded automatic setup input.
Only a `prompt_preview` with that flag should be excluded from the display title.
An explicit `provider_title` remains authoritative even if its text equals setup
text. Use the configured Agent title when a setup preview is excluded; no semantic
title generator or first actual user-task inference is implied. Keep these fields
with `description` as one snapshot when replaying metadata updates.

Owned Agent detail also exposes optional `first_message.purpose` (`setup` or
`task`) and `first_message.turn_sequence`. Missing purpose is legacy unknown;
Agent creation origin and user-role message position are not substitutes for it.
A Trigger task attached to a reused lifecycle Agent retains `task` purpose.

`usage_updated` contains absolute lifetime input/output consumption and
`turn_sequence`, `turn_input_tokens`, `turn_output_tokens` for the most recent
observed turn. The usage read also exposes optional `current_turn`. Match that
sequence to the running turn before displaying a current-turn counter. Context
compaction can reduce context size while lifetime counts remain unchanged.
Counts include cached input and advance only on provider reports; absent
historical usage is never inferred from message length. Retained older runtimes
continue emitting the compatible `usage` vocabulary.

## File attachments

`client.agents().upload_attachment(agent_id, name, media_type, content, &key).await` uploads one binary file of at most 20 MiB to an owner-authorized active Agent. Empty files are supported. Use `application/octet-stream` for an unknown media type. The returned `AgentAttachment` includes the verified persisted `path`, sanitized `name`, exact `size_bytes`, `media_type`, and `sha256`.

Uploading never sends a message or executes content. Refer to the returned path in the next Agent input. Retry an uncertain result with the same key and unchanged filename, type and bytes. Conflicting reuse is refused. Attachments require a supported runtime with full persistent home storage and are stored under `/home/attachments/<agent-id>/<attachment-id>/`; retained unsupported images and inactive Agents return typed errors. The existing approved-artifact APIs retain their separate publication contract.

## Message feedback

`agents().message_feedback(agent_id, EventOptions { after, limit })` reads private
owner votes as a `ResourcePage<MessageFeedback, u64>`. Pass each non-null `next`
as `after`; page size is 1–200. `set_message_feedback(agent_id, "event:42",
Some(MessageVote::Up))` sets a positive vote; use `MessageVote::Down` to switch or
`None` to remove it. The ID uses the transcript's durable message sequence.
These idempotent metadata writes need no command key and never create turns.
Serialize changes to one message and avoid replaying an old write after a newer
choice. Current owner authority is required; shared readers cannot inspect or
change these votes. New writes accept only the nonempty Assistant message with
the greatest durable sequence in a successfully completed turn. Other targets
return 404; historical votes remain readable, and the up/down/null request and
response shapes remain unchanged.

`agents().feedback_details(agent_id, "event:42")` returns `FeedbackDetails` with
viewer/Agent/answer identities, `version`, `reasons`, `comment`, `updated_at`,
and `request_id`. After confirming down, call `set_feedback_reasons(agent_id,
"event:42", &FeedbackReasonRequest { expected_version, request_id, reasons,
comment })`. `FeedbackReason` has six variants: `Inaccurate`, `Unanswered`,
`UnusableSteps`, `Incomplete`, `Unclear`, `Other`; selections must be distinct.
Use an empty optional comment or at most 500 Unicode code points, and a stable
ASCII request ID (1–128 letters/digits or `_.:-`). Exact retries recover the same
durable result; stale versions or changed request reuse return 409
`feedback_version_conflict`. Reread after an uncertain outcome. Up/null atomically
clears saved details for all clients, while retaining a version fence. No Agent
input or training operation is created.

### Agent session share links

`resource_access().create_agent_share_link(agent_id, &CreateAgentShareLinkRequest,
&key)` creates an Agent-only view or view-write link. Supply a cryptographically
random 32-byte token encoded as lowercase hex, and retain it in the share URL
fragment. The server stores only its digest. `AgentShareAudience` selects public,
organization, or canonical user subjects resolved with `share_recipients`.
`agent_share_links` returns metadata without secrets; `redeem_agent_share_link`
binds possession to the current signed-in recipient. `revoke_agent_share_link`
uses the current revision to invalidate this link while retaining independent
links. `EffectiveAccess.shared_by` carries public owner attribution.

### Persistent-home files

`client.agent_files().list(agent_id, "")` lists `/home/`, including hidden
entries. Pass a relative path to expand a directory. `read(agent_id, path,
max_bytes)` returns base64 content and its digest; `download(agent_id, path)`
returns raw bytes, with a dedicated 32 MiB transfer limit.

`write(agent_id, &AgentFileWrite)` uses the last-read `expected_sha256` for
compare-and-swap saves. Omission means create-only; `directory: Some(true)` with
empty content creates a folder. `remove(agent_id, path, recursive)` deletes one
target, recursively only when explicitly requested. File writes and
`redeem_agent_share_link(share_id, token)` need no idempotency key. No mutation
is automatically retried. Retain local edits after a conflict and reread before
an explicit retry. View links may browse/download; only owners and current
view-write links may change files. Older runtimes return
`file_runtime_unsupported` until updated while idle.

### Computer observation

`client.agent_files().environment(agent_id)` reads current feature availability,
reasons and scopes with a policy revision, opaque runtime incarnation and renewal
deadline. The SDK validates the Agent identity. Each later operation still
checks authorization. Files retain the mounted-home scope. Authorized operation
record replay is available with `operation_records_only`, `agent_turn_history`
and `writable: false`; browser and control require verified runtime capabilities
and explicit browser enablement.

Use `SnapshotOptions { tool_output: true, ..Default::default() }` and
`TailOptions { tool_output: true, ..Default::default() }` for Agent events v2, or
`agents().events_with_output(agent_id, options)` for legacy event reads. Reported
output is a bounded `{text, truncated}` replacement snapshot. Output-only events
do not carry a status. Responses v2 `ActivityItem.recorded_output` reads the
additive `cma:output` extension while the standard `output` remains absent.

Replay uses `agent_events_v2().snapshot(agent_id, options)` and the existing
opaque Agent and page cursors. Read no more than 200 events per page and retain
the fixed `through_cursor` while following `next_page_cursor`. A historical
record is identified by Agent, recorded turn sequence and event cursor. Fold
only through that cursor for its command, output, status, timestamp and preceding
plan; absent snapshots stay unknown. Fresh authorized reads restore a position
after reload. Keep live reception separate and use a fresh snapshot for Live.
Expose gaps and unavailable or expired positions; no expiry deadline is promised
by the retained event contract. These queries admit no command or control effects.

### Answer deliverables

`agents().answer_deliverables(agent_id, "event:42", None)` reads the explicitly
referenced resources in one completed final answer. Each item keeps its original
Agent, answer, turn, resource kind and version evidence. A live home file remains
a mutable file; an approved artifact and a document source revision retain their
independent identities. Ordinary filenames in prose do not create deliveries.

`describe_deliverable(agent_id, &DeliverableResource::LiveFile { path:
"report.csv".into() }, None)` verifies an explicit resource and returns a marker
for inclusion as a standalone line in the final answer. It does not publish files.
Document references use an explicit positive source revision. Agent runtime tools
return the same verified markers with publication receipts.

`read_answer_deliverable(agent_id, answer_id, delivery_id, true, None)` verifies
one item's bytes on an explicit retry. Metadata listing does not eagerly read
file bodies. `download_answer_deliverable(agent_id, answer_id, &item, None)`
returns opaque bytes only after checking the size, SHA-256 and current independent
resource authorization, including another authorization read after buffering.
Dropping the future cancels the operation; all download steps share one deadline.
Saving the bytes is the caller's choice. Changed live files cannot substitute for
the answer's original bytes; changed documents can still deliver their original
source revision. Pass a document share token only when independently authorized.

### Browser observation

Read `agent_files().environment(agent_id)` for the current opaque
`runtime_incarnation` and browser availability. The owner explicitly calls
`agent_browser().configure(agent_id, incarnation, true)` to enable that Agent's
browser in that runtime incarnation. Observation defaults off. Inspect the
returned state: a successful HTTP response can truthfully report `unsupported`.

`agent_browser().read(agent_id, incarnation, Some(observer), context)` returns a
bounded observation under current authorization. Pin the returned context,
reuse one observer per mounted viewer and stop reads on Agent, account or access
changes. Dropping the future cancels the read. Real frames expire after 3000 ms;
duplicates never renew freshness. Poll no faster than 1000 ms and call
`agent_browser().release(agent_id, incarnation, observer)` when hidden or closed.
Release preserves the page. Observation grants no input authority.

### Exclusive browser control

Use `agent_browser_control().read(agent_id, &scope)` to read current control and
its bounded audit, and `change(agent_id, &change)` for acquire, input, renew and
return. `BrowserControlScope` pins the environment incarnation, browser context,
unique client id and independent random UUID `client_key`. The SDK sends the
private key in `X-CMA-Browser-Client`, never in the query or JSON body.

Only a current owner may acquire, recover a selected receipt, input, renew or
return. Every mutation needs an original UUID operation id and the last observed
revision; input, renew and return also need the acquired lease token. The fixed
15-second lease uses a monotonic runtime clock and requires explicit renewal.
Inputs are viewport clicks, bounded text and supported keys without modifiers.
Native Agent input is fenced while the user holds control. Return preserves the
same browser context and page and creates no task or conversation input.

After a lost response, read the original operation or resend the identical
change. Do not issue another operation until its outcome is known. A receipt
contains both the original operation revision and current control state; it may
report an expired lease. Stop input/renewal on account, authority, incarnation,
offline or visibility changes. Public audit contains at most 128 metadata entries
and no input text, client keys or lease tokens. It is volatile per incarnation.
See `docs/plans/cma-ui-optimization/17-CONTROL-CONTRACT.md` for reader floors and
bounded revocation behavior.

## Binding event media continuations

A bound consumer Agent Key can publish an original event attachment, read its
completed receipt, and admit a separate supplementary response. It cannot select
another Agent or an arbitrary filesystem path. The current binding and Gateway
artifact authority are revalidated by the server on every operation.

Use `client.provider_bindings().upload_event_attachment(&target, &upload,
bytes)`, where `BindingMediaTarget` fixes the binding, conversation, event and
attachment index and `BindingMediaUpload` carries the frozen filename, original
SHA-256, MIME type, expiry and optional Gateway generation. The SDK enforces the
20 MiB limit. `event_attachment(&target)` reads its completed receipt without
republishing bytes. `supplement_event_media(&target, &body)` accepts a
`BindingMediaSupplement` containing the exact artifact and derived UTF-8 text
(at most 64 KiB), returning a separate command/response receipt.

These routes need no caller-generated idempotency key: the original parent event
and artifact slot identify the operation. Preserve the same slot, hashes, bytes
and text after an uncertain result. The SDK does not automatically retry or
create replacement events. A supplement uses Append ordering and leaves the
original response intact. A 202 receipt acknowledges admission, not completion;
follow its `response_url` through the Responses API. Typed media refusals retain
the stable server code and Retry-After metadata, including temporary source
unavailability and current authority revocation.

Gateway channel owners can read retained supplementary response metadata with
`gateway.get_event_media_responses(channel_id, event_id, &options)`.
Follow `next_cursor` until absent or null. Each page covers up to sixteen original
attachment slots and contains only consumer/response identities and status.
Activity snapshots also retain each consumer's optional `media_responses`.
These reads remain useful after the original event capability expires. They do
not grant access to response text: every returned Manager `response_url` still
requires existing Agent response authorization. The SDK never follows those URLs
or replays an event automatically.

## Retained provider associations


**Coverage:** `channel-usage` and its history describe the legacy explicit Bridge
usage projection only. Provider binding/conversation admission does not populate
that projection. An empty result does not establish that the Agent was never
associated with a provider, and must not be used as deletion eligibility. Use
`provider-associations` for retained Manager membership; neither route invents
missing event-use history.


`agents().provider_associations(id, &page)` reads retained membership under
current Agent management authority. Retired and borrowed associations remain
visible without changing Agent creation origin. Parent binding state and
generation describe the database snapshot for that page; subsequent pages use
fresh snapshots. Reservation timestamps do not represent message usage. Keep
continuations with their exact Agent incarnation and restart traversal when a
fresh full view is needed.

```rust
use cma_sdk::{Client, PageOptions};
async fn memberships(client: &Client, agent_id: &str) -> cma_sdk::Result<()> {
    let mut options = PageOptions { limit: Some(50), cursor: None };
    loop {
        let page = client.agents().provider_associations(agent_id, &options).await?;
        for item in page.items {
            println!("{} {}", item.binding_id, item.binding_generation);
        }
        match page.next {
            Some(next) => options.cursor = Some(next),
            None => break,
        }
    }
    Ok(())
}
```
