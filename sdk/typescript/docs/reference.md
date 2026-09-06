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
| `triggerAuthoring`                                      | `list`, `read`, `create`, `save`, `delete`, `publish`, `retire`, `listRevisions`, `listPlacements`, `createPlacement`, `retirePlacement`, `embed`, `stats`            |
| `launches`                                              | `preview`, `createOrResume`, `read`, `list`, `start`, `cancel`, `requirements`, `events`, `stream`, `createConnection`, `readConnection`                              |
| `agui`                                                  | `run`, `replay`                                                                                                                                                       |
| `admin`                                                 | `listSandboxes`, `readSandbox`, `sandboxEvents`, `retireSandbox`, `listProfiles`, `retireProfile`, `readSettings`, `replaceSettings`, `listActivity`                  |

`readConformance(options?)` performs `GET /api/v1/conformance` anonymously.

The `admin` namespace is a documented subset of the public REST administration
surface. Runtime inventory, settings patch/reset and Trigger pause/resume are
not SDK exports in this version; use their documented REST contracts when
needed. Do not infer extra methods from an operator's available permissions.

`readDefaults(options?)` performs the authenticated `GET /api/v1/defaults` and
returns only `sandbox_profile`, `workspace_profile`, and `agent_profile`, each
an immutable `{ id, revision }` reference. It requires `use_own_sandboxes`.
Read those exact revisions with the matching profile client's `readRevision`;
never infer configured defaults from Marketplace listings or replace a pinned
revision with the latest. Immutable revision reads admit the owner's profiles
and non-retired public profiles; draft access remains owner-management-only.

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
nonnegative safe integers and exact success statuses. Unknown resource-response
fields are omitted; an unknown discriminator in a closed resource-event union is
a `ProtocolError`. AG-UI preserves extension fields as described below.

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

## Trigger Authoring

`triggerAuthoring` manages the publisher's Draft and immutable publications. Every
target is an exact profile `{ id, revision }`, and every target profile must be
public at publication. Saving a Draft never changes the published Trigger. The
server validates reuse combinations, limits and publication eligibility; keep
typed field refusals beside the edited field.

```ts
import { createClient, type TriggerContent } from '@chronoai/cma-sdk';

const cma = createClient();
export async function saveEditedTrigger(
  id: string,
  content: TriggerContent,
  observedVersion: number,
  commandKey: string,
) {
  return cma.triggerAuthoring.save(
    id,
    { content, draft_version: observedVersion },
    { idempotencyKey: commandKey },
  );
}

export async function publishReviewedTrigger(id: string, savedVersion: number, commandKey: string) {
  return cma.triggerAuthoring.publish(
    id,
    { draft_version: savedVersion },
    { idempotencyKey: commandKey },
  );
}
```

Save returns a `TriggerDocument`; Publish returns a `PublishedTriggerRevision`
with the publication's `trigger_id`, `public_id`, revision, digest and content.
Use the Save result's `draft_version` for the separate Publish command. Retain
that version and the Publish key if its outcome is unknown.

Placements are stable identities, separate from their HTTPS `source_url` and
human `label`. `listPlacements(id, { includeRetired: true })` includes retired
records. `retirePlacement(id, placementId, options)` retires a placement without
rewriting its historical attribution.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function addReviewedPlacement(triggerId: string, commandKey: string) {
  const placement = await cma.triggerAuthoring.createPlacement(
    triggerId,
    { source_url: 'https://example.com/project', label: 'Project README' },
    { idempotencyKey: commandKey },
  );
  const embed = await cma.triggerAuthoring.embed(triggerId, placement.id);
  return { placement, embed };
}
```

The embed response owns every Markdown, HTML, JavaScript, React and URL snippet,
plus `javascript_sri` and the `content_security_policy.img_src` / `script_src`
directives. Display and copy these exact values; do not reconstruct them from
the public id. `stats` reports totals and placement attribution. A failed stats
read is unavailable data, never an observed zero.

## Visitor Launches

A public GET does not imply consent. Call admission only from a visitor's explicit
gesture or its already-saved bounded intent after sign-in:

```ts
import { createClient, repeatChoiceFrom, type CreateLaunchRequest } from '@chronoai/cma-sdk';

const cma = createClient();
export async function admitVisitorIntent(
  publicId: string,
  body: CreateLaunchRequest,
  commandKey: string,
) {
  try {
    return {
      record: await cma.launches.createOrResume(publicId, body, { idempotencyKey: commandKey }),
    };
  } catch (error) {
    const existing = repeatChoiceFrom(error);
    if (existing !== undefined) return { existing };
    throw error;
  }
}
```

The visitor command uses `start_if_ready: true`, optional `placement_id` and
`referrer_origin`, and an optional explicit `choice: 'resume' | 'new'`. The SDK
accepts HTTP 201 and 202 and returns a `Launch` for both. Inspect `state` and
`review.needs`: Draft opens only its requested review, while accepted work opens
progress. Repeated admission with the same key returns the recorded response;
it is not a way to refresh or advance an existing Draft.

After explicit trust review or satisfied account setup, continue that same Draft
with `start(id, { trust_confirmed_digest, acknowledgements }, options)`. Show the
recorded commands and environment values verbatim, and submit the exact digest
the visitor approved. Start returns HTTP 202 and refreshes authoritative account
requirements. `requirements(id)` is a read with optional `display_name`,
`connection_mode` and `setup_url`; it does not itself advance the stored Launch.

For hosted setup, open a blank popup synchronously from the gesture, then call
`createConnection({ service_id, launch_id, intent_id }, options)`. Validate its
HTTPS `hosted_url` before navigating the popup. Read `readConnection(handoff_id)`
with bounded polling, verify the Launch and service identities, and continue via
Start only when its state is `satisfied`. `expired`, `cancelled` and `failed` are
separate outcomes. A manual or unknown `connection_mode` does not authorize
inventing a hosted handoff; use only the supplied manual setup metadata.

JSON progress pages come from `events(id, { after, limit })`, returning
`{ launch_id, events, next_cursor }`. The SSE companion uses the same durable
sequence but delivers individual `LaunchEvent` values. Follow with a bounded
signal, retaining an acknowledged cursor and last successful record:

```ts
import { createClient, ProtocolError, type Launch } from '@chronoai/cma-sdk';

const cma = createClient();
export async function followPreparation(
  id: string,
  signal: AbortSignal,
  showRecordedProgress: (record: Launch) => Promise<void>,
) {
  let record = await cma.launches.read(id, { signal });
  await showRecordedProgress(record);
  await cma.launches.stream(id, {
    after: record.event_head,
    signal,
    onEvent: async (event) => {
      const next = await cma.launches.read(id, { signal });
      if (next.event_head < event.seq)
        throw new ProtocolError('Progress snapshot is behind its event');
      await showRecordedProgress(next);
      record = next;
    },
  });
  return record;
}
```

`stream` makes one connection. Supply bounded reconnect and polling fallback in
the host application; do not acknowledge an event until its snapshot or durable
projection has actually been applied. A `ready` record's Agent resolution gives
the visitor's Agent id. Navigate there without submitting the first message again.
`cancel(id, options)` is an explicit command; `failure.retryable` is not a public
retry operation. Retryable worker work resumes under server control. Starting
over requires a new explicit intent and new key.

## Agent Conversations

`agui.run` posts once to `/api/v1/agui/run`, and `agui.replay` reads
`/api/v1/agui/events`. In both, `threadId` is an existing Agent id, not a new Chat
identity. A run request supplies user messages or resolved approval interrupts.

```ts
import { createClient, type AgentConversationEvent } from '@chronoai/cma-sdk';

const cma = createClient();
export async function sendExplicitMessage(
  agentId: string,
  text: string,
  signal: AbortSignal,
  apply: (event: AgentConversationEvent) => Promise<void>,
) {
  await cma.agui.run(
    { threadId: agentId, messages: [{ role: 'user', content: text }] },
    { signal, onEvent: apply },
  );
}

export async function recoverConversation(
  agentId: string,
  acknowledgedCursor: number,
  signal: AbortSignal,
  apply: (event: AgentConversationEvent) => Promise<void>,
) {
  await cma.agui.replay(agentId, {
    after: acknowledgedCursor,
    follow: true,
    maxReconnects: 4,
    signal,
    onEvent: apply,
  });
}
```

Frames preserve protocol `type`, `threadId`, `runId` and extension fields. The
SDK validates frame identity, SSE framing and cursor boundaries, but does not
replace an application's event-specific reducer: extension values are `unknown`.
One durable event may project several frames; only its final frame carries
`cursor`. The SDK buffers that projection and advances replay only after all
callbacks resolve. Store a cursor only after applying the corresponding frame.
Live frames carry `cma:live: true` and no durable cursor; they are supplemental
content and never replace recorded history.

`replay(agentId, { follow: false, onEvent })` reads a finite captured history.
Following replay reconnects with bounded
backoff, defaults to four consecutive no-progress reconnects, and accepts a limit
from zero through twenty. A successful durable acknowledgement resets that
count. `run` never reconnects its POST. `timeoutMs` bounds admission and response
headers, not an active AG-UI response body; use `signal` to bound the entire
operation. Replay transport failure does not change the durable run outcome.
