import type { CmaWebClient } from '../api/client';
import type { ResourceViewsClient } from './client';
import { conversationFromTranscript, foldAgentEvent } from '../agent/conversation';
import { mergeTranscriptSnapshot } from '../agent/chronology';

export class SharedAgentAccessChangedError extends Error {
  constructor() {
    super('Agent access changed. Refresh to check current access.');
  }
}

function accessDeadline(started: number, revalidateAt: string | null | undefined) {
  return Math.min(started + 55_000, revalidateAt == null ? Infinity : Date.parse(revalidateAt));
}

function assertDeadline(deadline: number, signal: AbortSignal) {
  signal.throwIfAborted();
  if (!Number.isFinite(deadline) || deadline <= Date.now())
    throw new SharedAgentAccessChangedError();
}

/** Agent-only reads; a narrow grant does not authorize ancestry or profile requests. */
export async function loadSharedAgent(
  web: CmaWebClient,
  client: ResourceViewsClient,
  agentId: string,
  signal: AbortSignal,
) {
  const started = Date.now();
  const access = await client.resourceAccess.effective('agent', agentId, { signal });
  if (!access.capabilities.includes('view_history'))
    throw new Error('Agent history is unavailable.');
  let deadline = accessDeadline(started, access.revalidate_at);
  assertDeadline(deadline, signal);
  const transcript = await web.readAgentTranscript(agentId, { signal });
  if (transcript.agentId !== agentId) throw new Error('Agent history identity changed.');
  const current = await client.resourceAccess.effective('agent', agentId, { signal });
  deadline = Math.min(deadline, accessDeadline(started, current.revalidate_at));
  assertDeadline(deadline, signal);
  if (
    current.policy_revision !== access.policy_revision ||
    !current.capabilities.includes('view_history')
  )
    throw new SharedAgentAccessChangedError();
  return {
    value: {
      agentId,
      conversation: conversationFromTranscript(transcript),
      access: current,
      deadline,
    },
    revalidateAt: new Date(deadline).toISOString(),
  };
}

/** Rebuild rich history offscreen; the already-authorized transcript remains visible. */
export async function hydrateSharedAgent(
  web: CmaWebClient,
  client: ResourceViewsClient,
  snapshot: Awaited<ReturnType<typeof loadSharedAgent>>['value'],
  signal: AbortSignal,
) {
  const { agentId, access, deadline } = snapshot;
  let conversation = conversationFromTranscript({ agentId, turns: [], throughSeq: 0 });
  let cursor: number | undefined = 0;
  do {
    assertDeadline(deadline, signal);
    const page = await web.readAgentEvents(agentId, { cursor, signal });
    assertDeadline(deadline, signal);
    conversation = page.items.reduce(foldAgentEvent, conversation);
    if (page.next !== undefined && page.next <= (cursor ?? 0))
      throw new Error('Agent history pagination did not advance.');
    cursor = page.next;
  } while (cursor !== undefined);
  conversation = mergeTranscriptSnapshot(
    conversation,
    snapshot.conversation,
    conversation.cursor,
    true,
  );
  const current = await client.resourceAccess.effective('agent', agentId, { signal });
  assertDeadline(Math.min(deadline, accessDeadline(Date.now(), current.revalidate_at)), signal);
  if (
    current.policy_revision !== access.policy_revision ||
    !current.capabilities.includes('view_history')
  )
    throw new SharedAgentAccessChangedError();
  return conversation;
}
