import { beforeEach, expect, it, vi } from 'vitest';
import { agentTransportJournal, clearAgentTransport } from './journal';

beforeEach(() => sessionStorage.clear());
const intent = {
  key: 'original-key',
  responseId: 'resp_original',
  commandId: null,
  body: { source_command_id: 'cmd_original', expected_command_revision: 7 },
};

it('preserves existing input/control wire bytes and recovers the exact cancellation after reload', () => {
  const first = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  first.select(true);
  const input = first.retain('Keep this pending input');
  const keys = Object.keys(sessionStorage);
  const bytes = keys.map((key) => sessionStorage.getItem(key));
  first.queuedCancellation.retain(intent);
  expect(keys.map((key) => sessionStorage.getItem(key))).toEqual(bytes);
  const restarted = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  expect(restarted.read()).toEqual(input);
  expect(restarted.queuedCancellation.read()).toEqual(intent);
  expect(() => restarted.queuedCancellation.retain({ ...intent, responseId: 'resp_new' })).toThrow(
    'Recover',
  );
  restarted.queuedCancellation.accepted(intent.key, 'cancel_receipt');
  expect(first.queuedCancellation.read()?.commandId).toBe('cancel_receipt');
  expect(() => first.queuedCancellation.accepted(intent.key, 'foreign')).toThrow('does not match');
  expect(() => first.queuedCancellation.refused(intent.key)).toThrow('does not match');
});

it('clears cancellation storage on logout and rejects old account handles', () => {
  const alice = agentTransportJournal(sessionStorage, 'alice', 'agent-one');
  alice.select(true);
  alice.queuedCancellation.retain(intent);
  clearAgentTransport(sessionStorage);
  const bob = agentTransportJournal(sessionStorage, 'bob', 'agent-one');
  bob.select(true);
  expect(bob.queuedCancellation.read()).toBeNull();
  expect(() => alice.queuedCancellation.accepted(intent.key, 'late')).toThrow('sign-in changed');
  expect(bob.queuedCancellation.read()).toBeNull();
});

it('refuses storage failures, corrupt bytes and stale envelopes without replacing the original intent', () => {
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
    throw new Error('full');
  });
  expect(() => journal.queuedCancellation.retain(intent)).toThrow('full');
  expect(journal.queuedCancellation.read()).toBeNull();
  journal.queuedCancellation.retain(intent);
  const key = Object.keys(sessionStorage).find((key) => key.endsWith(':queued-cancellation:v1'))!;
  const bytes = sessionStorage.getItem(key)!;
  for (const bad of [
    '{',
    JSON.stringify({ ...JSON.parse(bytes), epoch: 'old' }),
    JSON.stringify({ ...JSON.parse(bytes), extra: true }),
  ]) {
    sessionStorage.setItem(key, bad);
    expect(() => journal.queuedCancellation.retain(intent)).toThrow();
    expect(sessionStorage.getItem(key)).toBe(bad);
  }
});
