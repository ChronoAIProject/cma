# Sandboxes

<!-- nav: 4 Sandboxes -->

Sandbox lists include retained history by default. Set `excludeRetired: true` in the
list options to omit only `retired` records before pagination; keep the same filter
when following a cursor. Paused, failing, and retiring Sandboxes remain visible.
The option applies to personal, organization, and incoming shared inventories,
combines with the exact status filter, and does not delete any retained record.
The public query parameter is `exclude_retired=true`; false or omission preserves
the historical listing. Combining it with `status=retired` returns an empty page.

Agent Sandbox persists a Sandbox on its attached persistent volume. CMA does not
create, schedule, expose, download, or delete Sandbox snapshot archives. Pause,
revive, and retirement delegate runtime and volume lifecycle to Agent Sandbox;
the CMA lifecycle view reports only current transition eligibility and safe
operation progress.

A Trigger can cancel a pause before agent parking and teardown are admitted.
The `pause` operation reports `resume_writers` until the exact hold is released.
If teardown may already have started, the Launch waits for pause completion and
revives automatically. Navigation away does not cancel an admitted Launch.

A Sandbox pins an immutable Sandbox Profile when provisioned. Workspaces and
Agents belong to that Sandbox; profile edits do not change existing instances.
The default Sandbox list contains the caller's personal resources. Accessible
read methods also support current organization ownership and whole-Sandbox grants.

`grantServiceAccess(id, { services }, { idempotencyKey })` records explicit owner
authorization for additional NyxID services on a fully paused Sandbox. Original
profile pins and creation scope remain unchanged. `nyxid_services` returns the
effective service list, including these grants. Revival obtains a new scoped
credential before delivering it to the runtime; the owner's connected accounts
must still authorize every service. A new grant while Live returns HTTP 409
`sandbox_service_access_requires_pause`. Repeating an existing grant is a no-op,
including after revival. This operation only adds services; it does not revoke
existing access.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const page = await cma.sandboxes.list({ limit: 20, status: 'active' });
for (const sandbox of page.items) {
  const detail = await cma.sandboxes.read(sandbox.id);
  const workspaces = await cma.workspaces.list(sandbox.id);
  console.log(detail.id, workspaces.items);
}
if (page.next) await cma.sandboxes.list({ limit: 20, cursor: page.next, status: 'active' });
```

Creation requires an exact published `sandbox_profile: { id, revision }` and an
explicit idempotency key. Inspect the revision's software and environment
disclosures first. Where trust confirmation is required, submit the reviewed
`commands_digest` with `trust_confirmed: true`; do not automatically accept a new
digest after a refusal.

```ts
import { createClient, type CreateSandboxRequest } from '@chronoai/cma-sdk';

const cma = createClient();
export async function provision(reviewed: CreateSandboxRequest, commandKey: string) {
  return cma.sandboxes.create(reviewed, { idempotencyKey: commandKey });
}
```

Create returns HTTP 201 and a base view, not a ready runtime. `pause`, `revive` and
`delete` return accepted HTTP 202 views. Inspect `read` and `events` for recorded
progress. Do not replace resource identifiers or clear a conversation during
pause, revival or runtime rotation.

Read `lifecycle.pause` and `lifecycle.revive` for eligibility to begin a new
transition. An active Sandbox can still be temporarily busy: `reason` distinguishes
`checkpoint_in_progress` from `work_in_progress` while Agent Sandbox persists the
attached volume. `lifecycle.operation` records an admitted pause, revival or image
rotation phase without worker or writer credentials. Older servers can omit
`lifecycle`; treat that as unavailable eligibility, not permission inferred from
`status`. A command can race with new work and return HTTP 409
`sandbox_lifecycle_busy`; refresh before retrying. Idempotent command replays may
still succeed when a new transition is not eligible.

Use `workspaces.create(sandboxId, body, options)` to add a Workspace within the
pinned limits. `worktrees.list(workspaceId)` and `worktrees.read(id)` expose the
Agent's real working checkout; a Worktree is not an independent terminal surface.

The owner's `list`, `read` and `create` responses carry an optional `color`: the
colour every Agent in that Sandbox wears, spelled as lowercase `#rrggbb`. The
server assigns it and keeps it distinct from the owner's other live Sandboxes.
It is absent until assigned, on shared-grant reads and on older servers; show
your default colour then. Any other spelling is a `ProtocolError`.

See [Profiles](profiles.md) for revision selection and [Errors](errors.md) for
quota, trust and uncertain-command handling.

## Shared hierarchy

Use `sandboxes.readAccessible`, `workspaces.readAccessible` and
`worktrees.readAccessible` when the result can be shared. The return type preserves
the full personal detail or provides a safe shape marked `access: 'shared'`.
Shared shapes include authorized IDs, names, statuses, timestamps and counts,
without owner/profile configuration, repository paths, credentials or runtime
internals. The existing `read` methods keep their personal types and refuse shared
responses. `workspaces.listAccessible(sandboxId)` and
`worktrees.listAccessible(workspaceId)` provide the corresponding child unions.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function inspectSandbox(id: string) {
  const cma = createClient();
  const sandbox = await cma.sandboxes.readAccessible(id);
  if (sandbox.access === 'shared') return { id: sandbox.id, count: sandbox.agent_count };
  return { id: sandbox.id, count: sandbox.agent_count, profile: sandbox.profile };
}
```

A narrow Agent grant cannot reveal any parent or its counts. Each hierarchy read
requires current parent visibility; revoked, expired or unavailable authority
can refuse a later request. The default Sandbox collection excludes explicit
organization ownership before pagination; use `listAccessible({ organization })`
for explicit organization discovery.

## Organization and Shared with me inventory

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function organizationSandboxes(organization: string) {
  const cma = createClient();
  const page = await cma.sandboxes.listAccessible({ organization, limit: 20 });
  return page.items.map((sandbox) => ({ id: sandbox.id, agents: sandbox.agent_count }));
}
```

The organization must be a current canonical choice from `resourceAccess.organizations()`.
Each page obtains new membership evidence. Organization-owned resources and
whole-Sandbox grants are filtered before pagination; narrow Agent grants cannot
reveal a parent or its counts. Shared summaries include `access: 'shared'` and safe
metadata/counts. Personal owners retain the full personal summary. Omit the
organization and shared selectors for personal inventory. Existing `sandboxes.list()` retains its
personal type and refuses shared summaries. A changed policy or unavailable
membership yields a typed error without partial rows or a stale `next` cursor.

Set `shared: true` to discover incoming whole-Sandbox grants across all current
organizations: `cma.sandboxes.listAccessible({ shared: true, limit: 20 })`.
The server deduplicates Sandboxes and excludes your own personal resources before
pagination. Organization ownership alone does not enter this scope. Individually
shared Agents remain discoverable through `agents.provenanceInventory({ shared: true })`
without exposing their parents. `shared: true` and `organization` are mutually
exclusive. Discovery refuses an incomplete result if organization authority is
unavailable; each page revalidates all original membership evidence and grants.

## Runtime updates

Owner Sandbox reads may include `runtime_update`. Its `current_version` (nullable)
and `latest_version` are exact runtime image references, not semantic versions.
Use `can_update` for manual action eligibility and display `blocked_reason` when
present. Shared projections omit this owner configuration.

```ts
import { Cma } from '@chronoai/cma-sdk';

async function requestRuntimeUpdate(cma: Cma, id: string) {
  const detail = await cma.sandboxes.read(id);
  const update = detail.runtime_update;
  if (!update?.can_update) return;
  const body = {
    target_version:
      update.state === 'failed'
        ? (update.target_version ?? update.latest_version)
        : update.latest_version,
  };
  const key = crypto.randomUUID();
  // Keep this exact body and key if the result is uncertain; do not retarget a retry.
  return cma.sandboxes.updateRuntime(id, body, { idempotencyKey: key });
}
```

`updateRuntime` uses `POST /api/v1/sandboxes/{id}/runtime-update`. A 202 response
acknowledges scheduling; a 200 response may report an already-current runtime.
Both return a base Sandbox view with `runtime_update`. Continue reading the
existing detail to observe `queued`, `updating`, `failed` or `up_to_date` with a
recorded `completed_at`. Acceptance does not mean rotation has finished.

Automatic updates run daily at 02:00 Asia/Singapore. Active Agent turns, pending
work and approvals defer rotation until idle. When `preserves_home` is true the
same persistent home and retained conversations survive the update. A stale target
is rejected with 409 rather than silently updating to a different runtime. A
failed accepted update retains `target_version`; retry that target even if a
newer deployment has become available.

## Shared lifecycle commands

`submitCommand(id, { command, expected_revision }, { idempotencyKey })` admits an
explicit `pause`, `revive`, or `retire` intent and returns a safe receipt with HTTP 202. `readCommand(commandId)` recovers that receipt with HTTP 200. Shared detail
`revision` is optional for older-server compatibility; require it before submitting.

```ts
import { createClient } from '@chronoai/cma-sdk';

export async function requestPause(id: string, key: string) {
  const cma = createClient();
  const detail = await cma.sandboxes.readAccessible(id);
  if (detail.revision === undefined) throw new Error('Current Sandbox revision unavailable');
  const request = { command: 'pause', expected_revision: detail.revision } as const;
  const receipt = await cma.sandboxes.submitCommand(id, request, { idempotencyKey: key });
  return cma.sandboxes.readCommand(receipt.command_id);
}
```

Retain the request and key before submitting. If the response is uncertain, retry
that exact body and key, including its original revision. Do not call the example
again to derive a replacement revision. `state.status` is `queued`, `applied`, or
`rejected`. Applied records a committed transition (or one already scheduled or
satisfied); read current Sandbox status for runtime progress. Rejection supplies
a typed reason. Receipts expose no credential references or owner configuration.
The SDK rejects inconsistent receipt identities, revisions and terminal states.

Pause/revive require current `manage_sandbox`; retire requires owner/admin
`manage_sharing` plus the personal owner's deletion capability when applicable.
Shared recipients cannot retire the owner’s Sandbox. The server rechecks authority
before scheduling; receipt reads require the requesting actor and current Sandbox
visibility. The five-minute queue deadline never renews. HTTP 409 busy and HTTP
503 unavailable responses require reading/retrying; they do not establish success.
