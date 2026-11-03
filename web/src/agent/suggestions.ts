import type { ConversationState } from './conversation';

export interface SuggestedReplySet {
  readonly id: string;
  readonly viewerId: string;
  readonly agentId: string;
  readonly answerId: string;
  readonly scope: { readonly turn_sequence: number; readonly answer_sequence: number };
  readonly source: { readonly kind: 'template' | 'generated'; readonly version: string };
  readonly items: readonly { readonly id: string; readonly text: string; readonly rank: number }[];
}

export function scenarioSuggestions(
  conversation: ConversationState,
  viewerId: string,
  agentId: string,
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
