import type { ConversationState } from './conversation';
import { useMemo } from 'react';

import type { SuggestedReplySet } from '@chronoai/cma-sdk';
export type { SuggestedReplySet } from '@chronoai/cma-sdk';

export function useSuggestedReplies(
  conversation: Pick<ConversationState, 'messages' | 'activeRunId' | 'latestTurnId'> | undefined,
  viewerId: string,
  agentId: string,
  provided?: SuggestedReplySet | null,
) {
  const messages = conversation?.messages;
  const activeRunId = conversation?.activeRunId;
  const latestTurnId = conversation?.latestTurnId;
  return useMemo(
    () =>
      messages &&
      scenarioSuggestions({ messages, activeRunId, latestTurnId }, viewerId, agentId, provided),
    [messages, activeRunId, latestTurnId, viewerId, agentId, provided],
  );
}

export function scenarioSuggestions(
  conversation: Pick<ConversationState, 'messages' | 'activeRunId' | 'latestTurnId'>,
  viewerId: string,
  agentId: string,
  provided?: SuggestedReplySet | null,
): SuggestedReplySet | undefined {
  const answer = conversation.messages.at(-1);
  if (
    conversation.activeRunId ||
    !answer?.finalAnswer ||
    conversation.latestTurnId !== answer.runId ||
    answer.role !== 'assistant' ||
    answer.sequence === undefined ||
    answer.liveOnly ||
    answer.status !== 'complete' ||
    !answer.content.trim() ||
    !Number.isSafeInteger(Number(answer.runId)) ||
    Number(answer.runId) < 1
  )
    return undefined;
  const scope = { turn_sequence: Number(answer.runId), answer_sequence: answer.sequence };
  if (provided !== undefined) {
    if (
      !provided ||
      provided.viewerId !== viewerId ||
      provided.agentId !== agentId ||
      provided.answerId !== answer.id ||
      provided.scope?.turn_sequence !== scope.turn_sequence ||
      provided.scope?.answer_sequence !== scope.answer_sequence ||
      typeof provided.id !== 'string' ||
      !provided.id.trim() ||
      typeof provided.source?.version !== 'string' ||
      !provided.source.version.trim() ||
      !['template', 'generated'].includes(provided.source.kind) ||
      !Array.isArray(provided.items) ||
      provided.items.length === 0 ||
      provided.items.length > 3 ||
      provided.items.some(
        (item, index) =>
          !item ||
          typeof item.id !== 'string' ||
          !item.id.trim() ||
          typeof item.text !== 'string' ||
          !item.text.trim() ||
          new TextEncoder().encode(item.text).length > 256 * 1024 ||
          !Number.isSafeInteger(item.rank) ||
          item.rank < 0 ||
          (index > 0 && item.rank <= provided.items[index - 1]!.rank),
      ) ||
      new Set(provided.items.map((item) => item.id)).size !== provided.items.length
    )
      return undefined;
    return provided;
  }
  const prompt = conversation.messages.find(
    (message) => message.runId === answer.runId && message.role === 'user',
  );
  if (
    !prompt ||
    !/^\s*(?:please\s+)?compare\s+\S.+\s+(?:and|versus|vs\.?)\s+\S.+[.!?]?\s*$/i.test(
      prompt.content,
    )
  )
    return undefined;
  const version = 'comparison-v1';
  const id = JSON.stringify([
    viewerId,
    agentId,
    scope.turn_sequence,
    scope.answer_sequence,
    version,
  ]);
  return {
    id,
    viewerId,
    agentId,
    answerId: answer.id,
    scope,
    source: { kind: 'template', version },
    items: [
      {
        id: `${id}:criteria`,
        text: 'Add explicit comparison criteria and explain the trade-offs.',
        rank: 0,
      },
    ],
  };
}
