# Reference

<!-- nav: 5 Reference -->

`createClient(config)` returns a `Cma` with the following public resources:

| Resource                                                | Methods                                                                                                                                                               |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agentProfiles`, `workspaceProfiles`, `sandboxProfiles` | `list`, `read`, `create`, `save`, `publish`, `listRevisions`, `readRevision`, `setVisibility`, `retire`                                                               |
| `marketplace.agent`, `.workspace`, `.sandbox`           | `list`, `read`, `fork`                                                                                                                                                |
| `sandboxes`                                             | `list`, `read`, `create`, `pause`, `revive`, `delete`, `events`                                                                                                       |
| `workspaces`                                            | `list(sandboxId)`, `read`, `create(sandboxId, body, options)`, `delete`                                                                                               |
| `worktrees`                                             | `list(workspaceId)`, `read`                                                                                                                                           |
| `agents`                                                | `list`, `read`, `create(workspaceId, body, options)`, `input`, `steer`, `interrupt`, `answerApproval`, `stop`, `delete`, `events`, `transcript`, `usage`, `resources` |

`readConformance(options?)` performs `GET /api/v1/conformance` anonymously.

```ts
import { type Conformance, readConformance, resolveConfig } from '@chronoai/cma-sdk';

const config = resolveConfig({ baseUrl: 'https://cma.example.com' });
const deployment: Conformance = await readConformance(config, {
  requestId: 'deployment-check-1',
});
```

`Conformance` contains `service`, `status`, and an array of currently exposed
`interfaces`. Generic transport helpers remain available for diagnostics and
SDK development. Resource methods validate required fields, closed enums,
nonnegative safe integers and exact success statuses. Unknown response fields
are omitted; an unknown event discriminator is a `ProtocolError`.

Sandbox and Workspace creation return base views with HTTP 201. Use `read` for
expanded hierarchy detail. Their lifecycle commands return base views with
HTTP 202; acceptance does not mean the lifecycle has completed. Agent operations
likewise return HTTP 202, with only `input` carrying `{ queued: boolean }`.

Profile `save` and `publish` require the last observed `draft_version` for
compare-and-swap. `marketplace.*.read` returns a complete immutable revision and
resolved nested revisions; Sandbox entries also carry trust confirmation and
the commands digest. `fork` requires an explicit revision, keeping a retry
pinned to the revision the owner actually inspected.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const draft = await cma.agentProfiles.read('agp_default');
console.log(draft.draft.thinking_effort, draft.draft_version);

const accepted = await cma.agents.create(
  'workspace-id',
  {
    agent_profile: { id: 'agp_default', revision: 1 },
    first_message: 'Review the current changes.',
  },
  { idempotencyKey: 'create-agent-command-1' },
);
console.log(accepted.id, accepted.first_message?.state);
```

Examples use illustrative identifiers and command keys. A real caller supplies
an existing resource identifier and a fresh durable key for each new logical
command. It retains that key and the exact body until the outcome is settled.

Agent `resources` keeps unavailable numeric readings as `null`; missing
`context_window` in usage is distinct from a reported zero. Event and transcript
DTOs are discriminated unions. Durable histories do not manufacture live
reasoning or replace the public conversation stream.
