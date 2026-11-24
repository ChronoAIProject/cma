import type { Cma } from '@cma/client';
import type { ReadOptions } from '../client';
import type { ReplyPreview } from '../replyPreview';

export async function readReplyPreview(
  sdk: Cma,
  agentId: string,
  options?: ReadOptions,
): Promise<ReplyPreview> {
  const value = await sdk.agents.replyPreview(agentId, options);
  if (value.status === 'none')
    return {
      kind: 'none',
      agentId: value.agent_id,
      sourceThroughSeq: value.source_through_seq,
    };
  if (value.status === 'unavailable')
    return {
      kind: 'unavailable',
      agentId: value.agent_id,
      sourceThroughSeq: value.source_through_seq,
    };
  return {
    kind: 'ready',
    agentId: value.agent_id,
    sourceThroughSeq: value.source_through_seq,
    reply: {
      text: value.latest_assistant_reply.text,
      truncated: value.latest_assistant_reply.truncated,
      messageSequence: value.latest_assistant_reply.message_sequence,
      turnSequence: value.latest_assistant_reply.turn_sequence,
      appendedAt: value.latest_assistant_reply.appended_at,
    },
  };
}
