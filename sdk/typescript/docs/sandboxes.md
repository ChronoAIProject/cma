# Sandboxes

<!-- nav: 4 Sandboxes -->

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

Use `workspaces.create(sandboxId, body, options)` to add a Workspace within the
pinned limits. `worktrees.list(workspaceId)` and `worktrees.read(id)` expose the
Agent's real working checkout; a Worktree is not an independent terminal surface.

See [Profiles](profiles.md) for revision selection and [Errors](errors.md) for
quota, trust and uncertain-command handling.
