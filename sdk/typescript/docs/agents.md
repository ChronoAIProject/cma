# Agents

<!-- nav: 5 Agents -->

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
inconsistent ancestry is refused. Active channel bindings, channel-use history,
inventory filters and UI badges are separate work.
The existing source method continues to provide its published Trigger shortcuts.
