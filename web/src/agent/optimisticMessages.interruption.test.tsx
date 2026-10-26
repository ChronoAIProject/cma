import { renderHook } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useOptimisticMessages } from './optimisticMessages';
import { conversationFromTranscript, type ConversationState } from './conversation';
import { agentTransportJournal } from './transport/journal';
import type { AgentV2Session } from './transport/client';

beforeEach(() => sessionStorage.clear());

it('reconciles interruption input only with its new message command, never matching text or the cancellation', () => {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  journal.interruption.retain({
    key: 'interrupt-key',
    responseId: 'resp_old',
    receipt: null,
    body: { text: 'Same text', turn_sequence: 1, expected_agent_revision: 10 },
  });
  const session = { journal } as AgentV2Session;
  const identity = {};
  const empty = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
  const { result, rerender } = renderHook(
    ({ conversation }: { conversation: ConversationState }) =>
      useOptimisticMessages(identity, true, conversation, session),
    { initialProps: { conversation: empty } },
  );
  expect(result.current.messages).toMatchObject([{ state: 'unconfirmed', source: 'interrupt' }]);
  journal.interruption.accepted('interrupt-key', {
    controlId: 'cmd_cancel',
    commandId: 'cmd_new',
    responseId: 'resp_new',
  });
  for (const commandId of ['cmd_old', 'cmd_cancel']) {
    rerender({
      conversation: {
        ...empty,
        messages: [
          {
            id: commandId,
            role: 'user',
            content: 'Same text',
            status: 'complete',
            commandId,
          },
        ],
      },
    });
    expect(result.current.messages).toMatchObject([
      {
        content: 'Same text',
        state: 'accepted',
        receipt: { commandId: 'cmd_new' },
      },
    ]);
  }
  rerender({
    conversation: {
      ...empty,
      messages: [
        {
          id: 'new',
          role: 'user',
          content: 'Same text',
          status: 'complete',
          commandId: 'cmd_new',
        },
      ],
    },
  });
  expect(result.current.messages).toEqual([]);
});
