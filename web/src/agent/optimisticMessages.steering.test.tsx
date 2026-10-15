import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useOptimisticMessages } from './optimisticMessages';
import { conversationFromTranscript, type ConversationState } from './conversation';
import { agentTransportJournal } from './transport/journal';
import type { AgentV2Session } from './transport/client';

beforeEach(() => sessionStorage.clear());
const empty = () => conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
function setup() {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  const input = journal.retain('Original turn input');
  journal.accepted(input.key, { responseId: 'resp_1', commandId: 'cmd_input' });
  const session = { journal } as AgentV2Session;
  const identity = {};
  return { journal, session, identity };
}
const steer = (key = 'steer_key', text = 'Steering followup') => ({
  key,
  responseId: 'resp_1',
  commandId: null,
  body: { turn_sequence: 1, expected_agent_revision: 7, command: { type: 'steer' as const, text } },
});

it('retains lost steering acknowledgement separately from the input journal and reconciles its exact command after recovery', () => {
  const { journal, session, identity } = setup();
  const input = journal.read();
  const { result, rerender } = renderHook(
    ({ conversation }: { conversation: ConversationState }) =>
      useOptimisticMessages(identity, true, conversation, session),
    { initialProps: { conversation: empty() } },
  );
  let pending!: ReturnType<typeof result.current.begin>;
  act(() => {
    pending = result.current.begin('Steering followup', 'steer');
  });
  journal.retainControl(steer());
  act(() => pending.settled());
  expect(result.current.messages.filter((message) => message.source === 'steer')).toMatchObject([
    { content: 'Steering followup', key: 'steer_key', state: 'unconfirmed' },
  ]);
  journal.acceptedControl('steer_key', 'cmd_steer');
  rerender({ conversation: empty() });
  expect(result.current.messages.filter((message) => message.source === 'steer')).toMatchObject([
    { state: 'accepted', receipt: { commandId: 'cmd_steer', responseId: 'resp_1' } },
  ]);
  rerender({
    conversation: {
      ...empty(),
      messages: [
        {
          id: 'durable-steer',
          role: 'user',
          content: 'Steering followup',
          status: 'complete',
          commandId: 'cmd_steer',
        },
      ],
    },
  });
  expect(result.current.messages.filter((message) => message.source === 'steer')).toHaveLength(0);
  expect(result.current.messages).toMatchObject([
    { content: 'Original turn input', receipt: { commandId: 'cmd_input' } },
  ]);
  expect(journal.read()).toEqual(input);
});

it('restores pending steering text and its original key on a new visit without matching identical ordinary text', () => {
  const { journal, session, identity } = setup();
  journal.retainControl(steer('steer_key', 'Original turn input'));
  const { result, rerender } = renderHook(
    ({ conversation }: { conversation: ConversationState }) =>
      useOptimisticMessages(identity, true, conversation, session),
    { initialProps: { conversation: empty() } },
  );
  expect(result.current.messages).toHaveLength(2);
  expect(result.current.messages.find((message) => message.source === 'steer')).toMatchObject({
    key: 'steer_key',
    content: 'Original turn input',
    state: 'unconfirmed',
  });
  rerender({
    conversation: {
      ...empty(),
      messages: [
        {
          id: 'ordinary',
          role: 'user',
          content: 'Original turn input',
          status: 'complete',
          commandId: 'cmd_input',
        },
      ],
    },
  });
  expect(result.current.messages).toHaveLength(1);
  expect(result.current.messages[0]?.source).toBe('steer');
});

it('keeps successive identical steering inputs until their own receipts appear', () => {
  const { journal, session, identity } = setup();
  journal.retainControl(steer('first'));
  journal.acceptedControl('first', 'cmd_first');
  const { result, rerender } = renderHook(
    ({ conversation }: { conversation: ConversationState }) =>
      useOptimisticMessages(identity, true, conversation, session),
    { initialProps: { conversation: empty() } },
  );
  let pending!: ReturnType<typeof result.current.begin>;
  act(() => {
    pending = result.current.begin('Steering followup', 'steer');
  });
  journal.retainControl(steer('second'));
  act(() => pending.settled());
  journal.acceptedControl('second', 'cmd_second');
  rerender({ conversation: empty() });
  expect(result.current.messages.filter((message) => message.source === 'steer')).toHaveLength(2);
  rerender({
    conversation: {
      ...empty(),
      messages: [
        {
          id: 'first',
          role: 'user',
          content: 'Steering followup',
          status: 'complete',
          commandId: 'cmd_first',
        },
      ],
    },
  });
  expect(result.current.messages.filter((message) => message.source === 'steer')).toMatchObject([
    { receipt: { commandId: 'cmd_second' } },
  ]);
});

it('marks a locally retained steer as not sent only when explicit recovery removes its refused intent', () => {
  const { journal, session, identity } = setup();
  const { result, rerender } = renderHook(() =>
    useOptimisticMessages(identity, true, empty(), session),
  );
  let pending!: ReturnType<typeof result.current.begin>;
  act(() => {
    pending = result.current.begin('Steering followup', 'steer');
  });
  journal.retainControl(steer());
  act(() => pending.settled());
  journal.refusedControl('steer_key');
  rerender();
  expect(result.current.messages.find((message) => message.source === 'steer')).toMatchObject({
    content: 'Steering followup',
    state: 'not_sent',
  });
});

it('keeps a retained steering row while the session is temporarily unavailable', () => {
  const { journal, session, identity } = setup();
  const { result, rerender } = renderHook(
    ({ transportSession }: { transportSession: AgentV2Session | undefined }) =>
      useOptimisticMessages(identity, true, empty(), transportSession),
    { initialProps: { transportSession: session as AgentV2Session | undefined } },
  );
  let pending!: ReturnType<typeof result.current.begin>;
  act(() => {
    pending = result.current.begin('Steering followup', 'steer');
  });
  journal.retainControl(steer());
  act(() => pending.settled());

  rerender({ transportSession: undefined });
  expect(result.current.messages.find((message) => message.source === 'steer')).toMatchObject({
    content: 'Steering followup',
    state: 'unconfirmed',
  });
});
