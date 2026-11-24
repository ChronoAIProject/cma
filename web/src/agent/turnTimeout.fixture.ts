import { conversationFromTranscript } from './conversation';

export const turnTimeoutAt = '2026-01-01T00:00:00.000Z';
export function turnTimeoutFixture(content = 'Check the draft') {
  return conversationFromTranscript({
    agentId: 'agent-fixture',
    throughSeq: 4,
    turns: [
      {
        turnSequence: 1,
        startedAt: turnTimeoutAt,
        ownerMessage: content,
        agentMessages: ['Partial answer'],
        messages: [
          { sequence: 2, role: 'user', content, appendedAt: turnTimeoutAt },
          { sequence: 3, role: 'assistant', content: 'Partial answer', appendedAt: turnTimeoutAt },
        ],
        outcome: {
          type: 'failed',
          failure: { class: 'agent_turn_timeout', detail: '' },
          at: turnTimeoutAt,
        },
      },
    ],
  });
}
