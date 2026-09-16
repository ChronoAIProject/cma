import { beforeEach, expect, it, vi } from 'vitest';
import { agentTransportJournal, clearAgentTransport } from './journal';

const receipt = { responseId: 'resp_retained', commandId: 'cmd_retained' };
beforeEach(() => sessionStorage.clear());

it('recovers a lost POST reply after reload with the original input and key', () => {
  const first = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  expect(first.select(true)).toBe('responses_v2');
  const sent = first.retain('First line\nSecond line 🛰️');
  const restarted = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  expect(restarted.select(false)).toBe('responses_v2');
  expect(restarted.retain(sent.input)).toEqual(sent);
  expect(() => restarted.retain('A changed message')).toThrow('Recover your pending message');
  restarted.accepted(sent.key, receipt);
  expect(first.read()).toEqual({ ...sent, state: 'accepted', receipt });
  expect(restarted.retain('Next intentional message').key).not.toBe(sent.key);
});

it('keeps old tabs on AG-UI across flag reversal and separates Agent selection', () => {
  const old = agentTransportJournal(sessionStorage, 'alice', 'old-agent');
  expect(old.select(false)).toBe('agui');
  expect(agentTransportJournal(sessionStorage, 'alice', 'old-agent').select(true)).toBe('agui');
  const newer = agentTransportJournal(sessionStorage, 'alice', 'new-agent');
  expect(newer.select(true)).toBe('responses_v2');
  expect(() => old.retain('No mixed submission')).toThrow('has not selected');
});

it('retains a receipt before draft acknowledgement and refuses conflicting late receipts', () => {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const input = journal.retain('hello');
  expect(() => journal.accepted('wrong-key', receipt)).toThrow('retained message');
  journal.accepted(input.key, receipt);
  journal.accepted(input.key, receipt);
  expect(() => journal.accepted(input.key, { ...receipt, responseId: 'another' })).toThrow(
    'changed identity',
  );
  expect(() => journal.refused(input.key)).toThrow('pending message');
  expect(journal.read()?.receipt).toEqual(receipt);
});

it('prevents stale callbacks after logout or another account signs in', () => {
  sessionStorage.setItem('unrelated-draft', 'keep');
  const alice = agentTransportJournal(sessionStorage, 'alice', 'same-agent');
  alice.select(true);
  const pending = alice.retain('private draft');
  clearAgentTransport(sessionStorage);
  const bob = agentTransportJournal(sessionStorage, 'bob', 'same-agent');
  expect(bob.read()).toBeNull();
  expect(bob.select(false)).toBe('agui');
  expect(() => alice.accepted(pending.key, receipt)).toThrow('sign-in changed');
  expect(sessionStorage.getItem('unrelated-draft')).toBe('keep');
  const aliceAgain = agentTransportJournal(sessionStorage, 'alice', 'same-agent');
  expect(aliceAgain.read()).toBeNull();
  expect(() => bob.select(true)).toThrow('sign-in changed');
});

it('cannot create a fresh request when retained bytes are corrupt or storage refuses a write', () => {
  const storage = {
    getItem: (key: string) => sessionStorage.getItem(key),
    setItem: vi.fn((key: string, value: string) => sessionStorage.setItem(key, value)),
    removeItem: (key: string) => sessionStorage.removeItem(key),
    key: (index: number) => sessionStorage.key(index),
    get length() {
      return sessionStorage.length;
    },
  };
  const journal = agentTransportJournal(storage, 'alice', 'agent-one');
  journal.select(true);
  storage.setItem.mockImplementationOnce(() => {
    throw new DOMException('Full', 'QuotaExceededError');
  });
  expect(() => journal.retain('Never sent')).toThrow('Full');
  expect(journal.read()).toBeNull();
  const pending = journal.retain('Retained before sending');
  storage.setItem.mockImplementationOnce(() => {
    throw new DOMException('Full', 'QuotaExceededError');
  });
  expect(() => journal.accepted(pending.key, receipt)).toThrow('Full');
  expect(journal.read()).toEqual(pending);
  const key = [...Array.from({ length: storage.length }, (_, index) => storage.key(index))].find(
    (key) => key?.endsWith(':alice:agent-one'),
  )!;
  sessionStorage.setItem(key, '{broken');
  expect(() => journal.retain('Replacement')).toThrow('could not be recovered');
  expect(() => journal.select(false)).toThrow('could not be recovered');
});

it('permits replacing a request only after its exact admission refusal', () => {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  journal.select(true);
  const pending = journal.retain('first');
  expect(() => journal.refused('foreign')).toThrow('pending message');
  expect(journal.read()).toEqual(pending);
  journal.refused(pending.key);
  expect(journal.read()).toBeNull();
  expect(journal.retain('corrected').key).not.toBe(pending.key);
});
