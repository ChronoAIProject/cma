import type { CmaWebClient } from '../api/client';
import type { ResourceViewsClient } from './client';
import { conversationFromTranscript, foldAgentEvent } from '../agent/conversation';
import { mergeTranscriptSnapshot } from '../agent/chronology';

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
  const deadline = Math.min(
    started + 55_000,
    access.revalidate_at == null ? Infinity : Date.parse(access.revalidate_at),
  );
  const transcript = await web.readAgentTranscript(agentId, { signal });
  if (transcript.agentId !== agentId) throw new Error('Agent history identity changed.');
  let conversation = conversationFromTranscript({ ...transcript, turns: [], throughSeq: 0 });
  let cursor: number | undefined = 0;
  do {
    const page = await web.readAgentEvents(agentId, { cursor, signal });
    signal.throwIfAborted();
    conversation = page.items.reduce(foldAgentEvent, conversation);
    if (page.next !== undefined && page.next <= (cursor ?? 0))
      throw new Error('Agent history pagination did not advance.');
    cursor = page.next;
  } while (cursor !== undefined);
  conversation = mergeTranscriptSnapshot(
    conversation,
    conversationFromTranscript(transcript),
    conversation.cursor,
    true,
  );
  const current = await client.resourceAccess.effective('agent', agentId, { signal });
  if (
    current.policy_revision !== access.policy_revision ||
    !current.capabilities.includes('view_history') ||
    deadline <= Date.now()
  )
    throw new Error('Agent access changed. Refresh to check current access.');
  return {
    value: { conversation, access: current },
    revalidateAt: new Date(deadline).toISOString(),
  };
}
