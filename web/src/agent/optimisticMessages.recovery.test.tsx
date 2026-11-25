import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useOptimisticMessages } from './optimisticMessages';
import { conversationFromTranscript } from './conversation';
import { agentTransportJournal, clearAgentTransport } from './transport/journal';
import type { AgentV2Session } from './transport/client';

beforeEach(() => sessionStorage.clear());
function setup() {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  const identity = {};
  const conversation = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
  const view = renderHook(() =>
    useOptimisticMessages(identity, true, conversation, { journal } as AgentV2Session),
  );
  let pending!: ReturnType<typeof view.result.current.begin>;
  act(() => {
    pending = view.result.current.begin('Retained request');
  });
  return { ...view, journal, pending };
}

it('replaces an unsubmitted local row after explicit retry without inventing acceptance', () => {
  const { result, journal, pending } = setup();
  act(() => pending.settled());
  expect(result.current.messages).toMatchObject([
    { state: 'not_sent', content: 'Retained request' },
  ]);
  let retry!: ReturnType<typeof result.current.begin>;
  act(() => {
    retry = result.current.begin('Retained request');
  });
  expect(result.current.messages).toMatchObject([{ state: 'sending' }]);
  const intent = journal.retain('Retained request');
  const receipt = { responseId: 'resp_1', commandId: 'cmd_1' };
  journal.accepted(intent.key, receipt);
  act(() => {
    retry.accepted(receipt);
    retry.settled();
  });
  expect(result.current.messages).toHaveLength(1);
  expect(result.current.messages[0]).toMatchObject({ state: 'accepted', key: intent.key, receipt });
});

it.each(['pending', 'invalidated', 'observed'] as const)(
  'preserves uncertainty when journal evidence is %s',
  (evidence) => {
    const { result, journal, pending } = setup();
    if (evidence === 'pending') journal.retain('Retained request');
    else if (evidence === 'invalidated') clearAgentTransport(sessionStorage);
    else act(() => pending.observed({ responseId: 'resp_1', commandId: 'cmd_1' }));
    act(() => pending.settled());
    expect(result.current.messages).toHaveLength(1);
    expect(result.current.messages[0]?.state).toBe('unconfirmed');
  },
);

it('replaces only a cancelled local attempt when an older accepted message has the same text', () => {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  const original = journal.retain('Retained request');
  const oldReceipt = { responseId: 'resp_1', commandId: 'cmd_1' };
  journal.accepted(original.key, oldReceipt);
  const { result, pending } = setup();
  act(() => {
    pending.cancelled();
    pending.settled();
  });
  expect(result.current.messages).toMatchObject([
    { state: 'accepted', key: original.key, receipt: oldReceipt },
    { state: 'not_sent', content: 'Retained request' },
  ]);
  act(() => {
    result.current.begin('Retained request');
  });
  expect(result.current.messages).toMatchObject([
    { state: 'accepted', key: original.key, receipt: oldReceipt },
    { state: 'sending', content: 'Retained request' },
  ]);
  expect(journal.read()).toMatchObject({
    key: original.key,
    state: 'accepted',
    receipt: oldReceipt,
  });
});
