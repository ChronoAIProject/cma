# Agents

<!-- nav: 5 Agents -->

Read saved contextual follow-ups with
`client.agents.suggestions(agentId, { turn_sequence: 3, answer_sequence: 5 })`.
This is a read-only GET. `ready` contains the shared `SuggestedReplySet` with
stable identity, exact text, ranks and generation strategy version; other states
hide the group without blocking the answer or manual composer. Preserve its
`scope` as `answer_context` for an unchanged suggestion's normal conditional
send. Generation observations report token usage and latency; missing cost is
null, never a zero-cost claim. Neither reading nor refreshing starts model work.

Use `agents.createLifecycle` to create an Agent from an Image Manager selection
and inline configuration. Omit `image` for the deployment default. Persist the
exact request and idempotency key before sending it; an uncertain response must
be retried with the same body and key.

```ts
import type { Cma, AgentLifecycleCreateRequest } from '@chronoai/cma-sdk';

export async function provisionAgent(client: Cma, idempotencyKey: string) {
  const request: AgentLifecycleCreateRequest = {
    name: 'Research',
    initial_message: 'Compare three tools and cite your sources.',
    profile: { permission_mode: 'guarded', reasoning_effort: 'medium' },
  };
  const receipt = await client.agents.createLifecycle(request, { idempotencyKey });
  return client.agents.readLifecycle(receipt.id);
}
```

The receipt acknowledges durable admission. Read `readLifecycle(receipt.id)`
and verify its exact `agent_id` and owned parent hierarchy before opening the
preparing conversation. Operation `succeeded` means preparation, not task
completion. A phase event whose status is `ready` is only one gate.
`waiting_for_connection` includes the
missing services and an owner-bound target for the connection flow. After the
owner verifies those connections, repeat the original `createLifecycle` call
with its exact request and key to resume the same operation and child resources.
`failed` is terminal; surface its safe error to the caller.

`initial_message` preserves exact text, including whitespace and newlines, in
the original idempotent request. It must contain non-whitespace text and fit
262144 UTF-8 bytes. The server queues it durably as `purpose: task` after
environment preparation, replacing the readiness prompt. Omission retains
legacy `setup` behavior. Do not combine it with `repository` or send a separate
first message after creation. Discover support in the served OpenAPI request
schema; older services cannot admit this field. Read Agent detail/transcript
for delivery, recovery and results.

Set `workspace_id` to add an Agent to an existing Workspace owned by the caller.
The Workspace must be ready. Omit `image` and `repository` because the selected
Workspace supplies both. This mode requires permission to use the Workspace;
permission to create a new Sandbox is unnecessary. The Agent inherits its parent
Sandbox's key scope. Omit `profile.permissions.services`, or provide a subset of
the inherited services; additional services are rejected and a subset does not
reduce the inherited scope. Reusing the same idempotency key with another
Workspace is a conflict.

One Agent belongs to one Worktree in a Workspace and Sandbox. Its REST,
AG-UI and Responses interfaces project the same conversation, not separate chats.
REST detail, transcript, events and usage also support current organization or
share authority. Creation provenance provides a separately authorized read
with private attribution available to current resource managers.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function inspectAgent(agentId: string) {
  const agent = await cma.agents.read(agentId);
  const transcript = await cma.agents.transcript(agentId);
  const usage = await cma.agents.usage(agentId);
  const resources = await cma.agents.resources(agentId);
  return { agent, transcript, usage, resources };
}
```

`agents.list` accepts `sandbox`, `workspace`, `status`, `cursor` and `limit`.
`agents.create(workspaceId, body, options)` takes a pinned Agent Profile and may
include a first message. The HTTP 201 result does not imply that message has
finished; inspect its recorded delivery state.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function send(agentId: string, text: string, commandKey: string) {
  return cma.agents.input(agentId, { text }, { idempotencyKey: commandKey });
}
```

Input returns HTTP 202 with `queued`. `steer`, `interrupt`, `answerApproval`,
`stop` and `delete` return empty HTTP 202 responses. Acceptance is not completion.
Retain each logical command's key and exact payload until its outcome is known.

While a Sandbox checkpoint is active, input is refused before admission with
HTTP 409 `sandbox_checkpoint_in_progress`. No prompt is queued for later execution.
Refresh the Sandbox and submit again after `lifecycle.checkpoint` clears. This
also applies to AG-UI and Responses input; paused conversations still revive
automatically, and durable Trigger provisioning retains its own retry flow.

Usage and resources preserve unavailable metrics: `null` is not zero, and a
missing context window is not a measured empty window. Transcript/event unions
reject unknown closed discriminators instead of silently dropping conversation
state. Durable history does not reproduce transient live reasoning.

Render each transcript turn's `messages` in its recorded order. Entries retain
the durable `sequence`, `role` (`user` or `assistant`), `content` and `appended_at`,
including steering admitted during an active turn. `owner_message` contains only
the initial input and `agent_messages` only assistant text; those summaries
cannot reconstruct an interleaved conversation. Merge event replay by message
sequence, not by turn id, since one turn may contain several owner inputs.

`agents.terminal(id)` returns the server's named refusal as `ApiError`; no
attachment ticket or terminal plane exists. Use [Streaming](streaming.md) for
live conversation traffic and durable replay.

# Organization share management

`client.resourceAccess` provides `effective`, `listShares`, `readShare`,
`createShare`, `updateShare` and `revokeShare`. Share kinds are `agent` and
`sandbox`; effective policy also accepts `workspace` and `worktree`.
Create with `{ target_org_id, mode: 'view' | 'org_roles' }`; update with
`{ expected_revision, mode }`; revoke with the current revision. Each mutation
requires explicit command options containing `idempotencyKey`. Retrying the
same command/key recovers its original receipt after an uncertain response.

The personal owner manages grants. Recipients cannot reshare. Lists contain
active direct grants; individual reads retain revoked records. Unsafe narrow
interactive grants are rejected unless current isolation or an explicit
compatible whole-Sandbox grant is proved. Choose `view` for ordinary Agent
sharing. Effective capabilities describe current policy and expire at their
revalidation deadline; later operations recheck authorization.

Use `agents.readAccessible(id)` for detail that may be shared. It returns the
existing `Agent` for its personal owner, or `SharedAgent` with `access: 'shared'`
for every other authorized reader. The shared response includes the Agent ID,
status, optional title/model and timestamps. Parent identifiers, paths, profiles,
credentials and native conversation identifiers are outside this projection.
`agents.read(id)` keeps its full personal-detail type and rejects a shared response.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function inspectAccessibleAgent(agentId: string) {
  const cma = createClient();
  const detail = await cma.agents.readAccessible(agentId);
  if (detail.access === 'shared') return { id: detail.id, title: detail.title };
  return { id: detail.id, workspace: detail.workspace.name };
}
```

Shared transcript and event reads retain messages and outcomes, with internal
metadata and diagnostic details removed. Events outside the reader's scope have
`kind: { type: 'access_redacted' }`; keep their sequence and advance pagination
normally. This is a response projection, not a changed durable log. Shared usage
is Agent-specific. `agents.resources(id)` requires independent Sandbox access,
because those counters include other Agents in the Sandbox.

Each request obtains current membership evidence. Buffered history and metrics
are refused if policy changes or evidence expires before release. Membership
outages return `organization_authority_unavailable`; inaccessible Agents return
`agent_not_found`. Live streams, artifacts and interactions remain
under development.

Use `resourceAccess.organizations()` to obtain current canonical scope choices.
`agents.listAccessible({ organization: orgId, limit, cursor })` returns personal
`AgentSummary` or safe `SharedAgent` items. Omitting `organization` preserves the
personal inventory; the existing `agents.list` method retains its personal type.
Organization choices do not authorize later reads: every page checks current
membership and shares. Overlapping grants yield one row, and only authorized
rows determine `next`. Parent filters require independent parent access.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function inspectOrganization(organization: string) {
  const cma = createClient();
  const choices = await cma.resourceAccess.organizations();
  const agents = await cma.agents.listAccessible({ organization, limit: 20 });
  const first = agents.items[0];
  const source = first ? await cma.agents.sourceAccessible(first.id) : undefined;
  return { choices: choices.items, agents: agents.items, next: agents.next, source };
}
```

A changed buffered page, including its hidden lookahead, is refused with
`resource_access_changed`. Restart the read under current access. Provider
outages return `organization_authority_unavailable`; an unavailable resource
store returns `resource_access_unavailable`. No partial page is returned.

`agents.sourceAccessible(id)` returns `AccessibleAgentSource`. Personal owners
retain full creation provenance and resolvable published Trigger shortcuts.
A shared response has `access: 'shared'`, `agent_id` and `origin.type` only:
`manual`, `workspace_profile` or `trigger`. It withholds private Launch/Trigger
IDs and shortcuts and rechecks access after IO. The existing `agents.source(id)`
method keeps its personal return type and rejects shared responses; choose the
accessible method when inspecting shared Agents.

`agents.provenance(id)` reads immutable creation facts from
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

`agents.provenanceInventory(options)` lists safe creation summaries from
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

```ts
import type { Cma } from '@chronoai/cma-sdk';
async function readCreationInventory(client: Cma, organization: string) {
  let cursor: string | undefined;
  do {
    const page = await client.agents.provenanceInventory({
      organization,
      creationOrigin: 'manual',
      limit: 50,
      cursor,
    });
    for (const row of page.items) console.log(row.agent_id, row.creation_origin);
    cursor = page.next;
  } while (cursor !== undefined);
}
```

## Approved artifacts

`agents.listArtifacts(agentId, { limit, cursor })`, `readArtifact(agentId, artifactId)`
and `downloadArtifact(agentId, artifactId)` read only content explicitly published
to that Agent. Downloads return opaque `Uint8Array` bytes and honor cancellation
through body consumption. Each request rechecks current access; narrow Agent
shares can read these approved artifacts without gaining parent or repository
file access. Metadata has the stable artifact/Agent IDs, safe name, media type,
byte count, SHA-256 and publication time.

An owner or current sharing manager can publish selected content once:

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function publishReview(agentId: string, idempotencyKey: string) {
  const cma = createClient();
  return cma.agents.publishArtifact(
    agentId,
    {
      name: 'Review.txt',
      media_type: 'text/plain',
      content_base64: 'cmV2aWV3',
    },
    { idempotencyKey },
  );
}
```

Publication requires explicit content approval. Keep the identical key and body
when its result is uncertain; there is no automatic resubmission. Files contain
1 byte through 1 MiB, use canonical standard base64, and have a display name of
at most 160 UTF-8 bytes without controls or path separators. Supported media
are plain text, Markdown, JSON, PDF, opaque binary, PNG, JPEG and WebP. Each Agent
can retain 256 immutable artifacts; pages contain at most 50. Publication does
not send an Agent input or read a runtime filesystem. Current and future readers
of the Agent can retrieve the published content. Revoked access prevents later
reads; previously downloaded copies cannot be recalled.

## Channel usage

`agents.channelUsage(agentId)` reads explicit active and historical channel facts
alongside the immutable creation origin. Its `channel_usage` summary contains
`active`, `has_history`, and nullable `first_used_at`/`last_used_at`. Attaching a
binding records history; the use timestamps remain null until a channel command
has been accepted. Detachment preserves the recorded history and use times.
Manual and Trigger Agents retain their original creation category.

Only current resource managers receive the optional `bindings` array, containing
binding/bridge/partition references, generation lineage, attachment and use times.
Shared viewers receive the safe summary without those private references.
`agents.channelUsageHistory(agentId, { limit, cursor })` requires current resource
management authority and returns an `agent_id`, ordered `items`, and optional
`next` cursor. The default page size is 50; accepted limits are 1 through 200.
Each item preserves the original actor, timestamp, reason, changed binding IDs,
nullable detachment time, and bounded before/after binding facts. SDK reads check
the requested Agent identity, consecutive revisions, and continuation integrity.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function inspectChannelUse(agentId: string) {
  const cma = createClient();
  const usage = await cma.agents.channelUsage(agentId);
  if (usage.bindings !== undefined) {
    const history = await cma.agents.channelUsageHistory(agentId, { limit: 25 });
    return { usage, history };
  }
  return { usage };
}
```

These methods are observations. Channel binding facts do not grant access and
cannot be written or cleared through these routes. Each read requires current
resource authority, including after membership or sharing changes.

`agents.provenanceInventory({ channelActive, channelHistory, creationOrigin,
shared, organization })` combines those safe facts with existing scoped creation
inventory. For example, `channelActive: false, channelHistory: true` selects
historically bound Agents whose bindings have all detached. The `channel_usage`
row field is optional only for compatibility with older servers. A server scans
at most 200 authorized candidates per request; keep following `next` even when a
filtered page is empty. Channel filters never relabel a Manual or Trigger Agent.

Resource access responses include an optional safe ownership summary, absent on
older servers. Personal ownership reports only whether the current caller owns
the resource; organization ownership supplies an ID for matching the caller's
current organization choices. Use the resource's current capabilities to display
access levels. Organization membership alone does not establish resource permissions.

## Live settings

`agents.settings(agentId)` reads the owner's desired `model`, `reasoning_effort`,
`permission_mode` and `pending`. `agents.updateSettings(agentId, body,
{ idempotencyKey })` supplies all three settings for an idle Agent. Retain the
exact body and key when an HTTP reply is uncertain. `pending: true` records
accepted desired settings; read again until runtime acknowledgement clears
`pending` before treating them as applied. The immutable creation profile is
preserved.

`agents.models(agentId)` returns `{ models }`, where each model has `id`, `label`
and `available`. Only select rows whose `available` flag is true: this reflects
both the Agent's admitted model service and current runtime support. The catalog
does not change settings; use `updateSettings` with the chosen model ID.

## Live skills

`agents.skills(agentId)` reads `{ items, pending, can_install, unavailable_reason? }`.
Each item contains `id`, exact `version`, `source: 'ornn'`, and a `status` of
`pending`, `installed`, or `failed`, with optional `name` and `error`.
Use `can_install` and `unavailable_reason` to present current runtime support and
readiness; current authorization and readiness are checked again during admission.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function addSkill(
  agentId: string,
  skillId: string,
  exactVersion: string,
  installationKey: string,
) {
  const cma = createClient();
  return cma.agents.installSkill(
    agentId,
    { id: skillId, version: exactVersion },
    { idempotencyKey: installationKey },
  );
}
```

Choose an explicit skill and version from `profileAuthoring.searchSkills`. A `202`
response records accepted installation work. Reuse the unchanged body and key
after an uncertain reply, then read `skills` again until the selected item is
`installed` or `failed`. Only `installed` confirms runtime acknowledgement.
Installation is additive and preserves the immutable creation profile.

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

`agents.accessRequests(agentId)` returns `{ items }` for the authenticated owner.
Each request has an `id`, exact `agent_id`, `services`, `reason` and durable `status`.
`approveAccessRequest` sends the owner's explicit service selection with a stable
idempotency key. `denyAccessRequest` sends an explicit denial with its own key.
Services contain 1–32 ASCII identifiers of 1–128 characters using letters, digits,
hyphens and underscores. Authentication and current ownership are checked by the
service; the SDK does not infer approval from an Agent message.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function approveServices(
  agentId: string,
  requestId: string,
  services: string[],
  decisionKey: string,
) {
  const cma = createClient();
  return cma.agents.approveAccessRequest(
    agentId,
    requestId,
    { services },
    { idempotencyKey: decisionKey },
  );
}
```

Retry an uncertain decision with the unchanged body and original key. The durable
service returns current progress on replay. `connecting` and `applying` include
`connection.target = { type: 'agent_access_request', id: requestId }` for the
owner's connection flow. Recheck that target if a connection expires while access
is being applied; reconnecting preserves the approved scope and current request.
`applying` means runtime verification is pending; only `succeeded` confirms applied
access. Read the list again after a connection return. `denied` and `failed` are
terminal outcomes; an optional `error` supplies safe failure detail. SDK validation rejects crossed Agent/request identities,
inconsistent connection targets, and duplicate list IDs.

An explicit retry after a failed access request is a new pending consent attempt.
Its optional `retry_of` names the preserved failed request. The Agent tool returns
new attempt parameters after failure; old-key replay continues to return the old
outcome. New attempts require fresh owner approval and never inherit an earlier
approval or runtime update.

To add or revoke services directly, call `agents.updateServiceAccess(agentId, body, options)` with `{ services, expected_services }`.
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

`client.agentAttachments.upload(agentId, file, { idempotencyKey, signal })` uploads one browser `File` or `Blob` to the owner's active Agent. Pass `name` when using a plain `Blob`. Any regular file type is accepted, including images; each file may contain up to 20 MiB, including empty files. The binary body retains its declared media type. Names are sanitized into an ASCII basename and each operation gets a distinct directory.

The result contains `id`, `agentId`, `name`, `path`, `size`, `mediaType` and `sha256`. The verified `path` is under `/home/attachments/<agent-id>/<attachment-id>/`. Include that path in your subsequent Agent input when ready; uploading never sends a message or executes content. A lost response may be retried with exactly the same key, filename, media type and bytes. Reusing a key for different content returns a conflict. Aborting an uncertain request does not guarantee the file was not committed; preserve the key for recovery.

Only current owner authority may upload. Shared conversation readers and delegated Agents cannot write files through this route. The runtime must prove a full persisted home and the attachment capability; older runtime images and inactive Agents return a typed refusal. Existing Sandboxes are not rotated by this feature. File storage lasts with the Sandbox's persisted home; the approved artifact API remains the separate publication and download interface.

## Message feedback

`agents.messageFeedback(agentId, { after, limit })` reads private owner votes as
`{ items, next }`. Pass each non-null `next` as `after` until it is null; page size
is 1–200. `agents.setMessageFeedback(agentId, 'event:42', 'up')` sets a positive
vote; use `'down'` to switch or `null` to remove it. The message ID uses the
transcript's durable message sequence. These idempotent metadata writes need no
command key and never create turns. Serialize changes to one message and avoid
replaying an old write after a newer choice. Current owner authority is required;
shared readers cannot inspect or change these votes. New writes accept only the
nonempty Assistant message with the greatest durable sequence in a successfully
completed turn. Other targets return 404; historical votes remain readable, and
the up/down/null request and response shapes remain unchanged.

### Agent session share links

`client.resourceAccess.createAgentShareLink(agentId, { audience, mode, token }, options)`
creates an Agent-only share link. `mode` is `view` or `view_write`. `audience` is
`{ kind: 'public' }`, `{ kind: 'organization', organization_id }`, or
`{ kind: 'users', subjects }`. Resolve display names to canonical user subjects
with `shareRecipients(agentId, search)` before creating a users link. The owner
must select an existing CMA user; arbitrary usernames are not identity proofs.

Generate `token` from 32 cryptographically random bytes encoded as 64 lowercase
hex characters. Store the secret only in the share URL fragment, never a query
parameter. Reuse the exact body and idempotency key on retry. The server retains
only a digest and never returns the secret. `listAgentShareLinks` returns active
link metadata. The signed-in recipient calls `redeemAgentShareLink(shareId,
token)` once, then uses ordinary Agent APIs. `revokeAgentShareLink` takes the
current revision and invalidates this link's recipients immediately; independent
links remain valid. `effective(...).shared_by` provides the owner's public
identity for a shared-session banner. No Agent link grants Sandbox lifecycle or
sharing-management access.

### Persistent-home files

`client.agentFiles.list(agentId, '')` lists `/home/`; pass a relative directory
path to lazily expand it. Hidden entries are included. `read(agentId, path,
{ maxBytes })` returns base64 content and its SHA-256; `download(agentId, path)`
returns bytes. Transfers are bounded to 32 MiB. File responses use their own
bounded transport; ordinary API responses retain their existing limit.

`write(agentId, { path, content_base64, expected_sha256 })` saves only if the
last-read digest matches. Omit `expected_sha256` for create-only uploads, or use
`directory: true` and empty content to create a folder. A conflict preserves
server contents; retain the local draft and reread before explicitly retrying.
`remove(agentId, path, recursive)` deletes only that target. These file mutations
need no command key and are never retried automatically. The owner or a current
view-write link may write; view links may browse and download. An old runtime
returns `file_runtime_unsupported` until it is updated while idle.
