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
The client reads only the caller's resources unless an operator helper is used.

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
