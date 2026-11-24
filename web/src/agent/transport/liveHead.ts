import type { V2AgentEventHead } from '@cma/client';
import { conversationFromTranscript } from '../conversation';
import { foldV2Item } from './projection';
import { withApprovalProjection } from './externalApprovalProjection';

/** A server-issued live baseline is independent of the historical snapshot prefix. */
export function conversationFromHead(head: V2AgentEventHead) {
  let conversation = conversationFromTranscript({
    agentId: head.agent_id,
    throughSeq: 0,
    turns: [],
  });
  if (head.externalApprovalProjection)
    conversation = withApprovalProjection(conversation, head.externalApprovalProjection);
  if (head.active_turn !== null) conversation = foldV2Item(conversation, head.active_turn);
  return {
    ...conversation,
    cursor: Number(head.through_cursor.split('.').at(-1)),
    agentCursor: head.through_cursor,
    agentIncarnation: head.incarnation,
  };
}
