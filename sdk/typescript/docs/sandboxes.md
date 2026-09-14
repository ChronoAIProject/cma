# Sandboxes

<!-- nav: 4 Sandboxes -->

`lifecycle.latest_snapshot` reports the latest completed snapshot's `epoch`,
`saved_at`, and owner-authenticated `download_url`. Fetch the relative URL against
the same CMA origin with the owner's credentials to download the whole-home
archive. Old epoch URLs return 404 after replacement; snapshots are never public.

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
`checkpoint_in_progress` from `work_in_progress`. `lifecycle.checkpoint` records
`capture` or `resume_writers`, its `started_at` timestamp and checkpoint `epoch`;
`lifecycle.operation` records an admitted pause, revival or image rotation phase.
These fields contain no worker or writer credentials. Older servers can omit
`lifecycle`; treat that as unavailable eligibility, not permission inferred from
`status`. A command can race with new work and return HTTP 409
`sandbox_lifecycle_busy`; refresh before retrying. Idempotent command replays may
still succeed when a new transition is not eligible.

Use `workspaces.create(sandboxId, body, options)` to add a Workspace within the
pinned limits. `worktrees.list(workspaceId)` and `worktrees.read(id)` expose the
Agent's real working checkout; a Worktree is not an independent terminal surface.

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
can refuse a later request. Snapshot archives remain personal-only. The default
Sandbox collection excludes explicit organization ownership before pagination;
use `listAccessible({ organization })` for explicit organization discovery.

## Organization inventory

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
organization for personal inventory. Existing `sandboxes.list()` retains its
personal type and refuses shared summaries. A changed policy or unavailable
membership yields a typed error without partial rows or a stale `next` cursor.
