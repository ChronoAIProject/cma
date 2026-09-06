# Agents

<!-- nav: 5 Agents -->

One Agent belongs to one Worktree in a Workspace and Sandbox. Its REST,
AG-UI and Responses interfaces project the same conversation, not separate chats.
All ordinary reads and commands are owner-scoped.

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
