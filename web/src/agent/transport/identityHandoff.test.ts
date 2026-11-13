import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { harness, item, json, page } from './controller.fixture';
import { agentTransportJournal } from './journal';
import { createIdentityHandoff, identityGateDiagnostics } from './identityGate';
import { loadV2Agent } from './load';
import { IdentityChangedError } from './identity';

vi.mock('./identityGate', async (original) => ({
  ...(await original<typeof import('./identityGate')>()),
  IDENTITY_HANDOFF_ENABLED: true,
}));

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it('consumes a proof once, rejects expired proofs, and reports only aggregate counts', () => {
  const now = vi.spyOn(performance, 'now').mockReturnValue(1000);
  const before = identityGateDiagnostics().handoffs;
  const consume = createIdentityHandoff(true);
  expect(consume()).toBe(true);
  expect(consume()).toBe(false);
  const expired = createIdentityHandoff(true);
  now.mockReturnValue(1251);
  expect(expired()).toBe(false);
  expect(identityGateDiagnostics().handoffs - before).toBe(1);
  expect(createIdentityHandoff(false)()).toBe(false);
});

it('opens a selected v2 Agent with exactly one check before the first data read', async () => {
  const h = harness();
  agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const read = vi.spyOn(h.client, 'readAgent').mockImplementation(async () => {
    expect(h.currentUser).toHaveBeenCalledOnce();
    return (await import('../../test/agent')).AGENT_DETAIL;
  });
  await loadV2Agent(h.client, 'agt_1', new AbortController().signal, session);
  expect(read).toHaveBeenCalledOnce();
  expect(h.eligibility).not.toHaveBeenCalled();
  expect(h.currentUser).toHaveBeenCalledTimes(2);
});

it('keeps the post-eligibility check on an unselected tab', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  expect(h.currentUser).toHaveBeenCalledTimes(2);
  await session.authorizeLoad(new AbortController().signal);
  expect(h.currentUser).toHaveBeenCalledTimes(2);
});

it('requires fresh identity after the entry proof is consumed or older than 250ms', async () => {
  const h = harness();
  agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
  const now = vi.spyOn(performance, 'now').mockReturnValue(1000);
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  now.mockReturnValue(1251);
  await session.authorizeLoad(new AbortController().signal);
  expect(h.currentUser).toHaveBeenCalledTimes(2);
  await session.authorizeLoad(new AbortController().signal);
  expect(h.currentUser).toHaveBeenCalledTimes(3);
});

it('invalidates the proof when preparation is handed to navigation', async () => {
  const h = harness();
  agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
  const prepared = h.client.agentTransport!.prepare('agt_1', 'alice');
  prepared.handoff();
  const session = (await h.client.agentTransport!.openForLoad('agt_1'))!;
  h.currentUser.mockResolvedValue('bob');
  await expect(
    loadV2Agent(h.client, 'agt_1', new AbortController().signal, session),
  ).rejects.toBeInstanceOf(IdentityChangedError);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
});

it('checks local identity even when a fresh handoff exists', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  agentTransportJournal(sessionStorage, 'bob', 'agt_1').select(true);
  await expect(session.authorizeLoad(new AbortController().signal)).rejects.toBeInstanceOf(
    IdentityChangedError,
  );
});

it('checks only once per additional history page when no publication intervenes', async () => {
  const h = harness();
  agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  for (let i = 1; i <= 10; i++) {
    h.snapshot.mockResolvedValueOnce(
      json(page([item(i, { type: 'conversation_available' })], 10, i < 10 ? i : undefined)),
    );
  }
  await loadV2Agent(h.client, 'agt_1', new AbortController().signal, session);
  expect(h.snapshot).toHaveBeenCalledTimes(10);
  // Selection, initial publication, then nine post-page checks.
  expect(h.currentUser).toHaveBeenCalledTimes(11);
});

it('does not read the next page or publish after a post-page check crosses accounts', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.snapshot.mockImplementationOnce(async () => {
    h.currentUser.mockResolvedValue('bob');
    return json(page([item(1, { type: 'conversation_available' })], 2, 1));
  });
  const publish = vi.fn();
  await expect(
    loadV2Agent(h.client, 'agt_1', new AbortController().signal, session, false, publish),
  ).rejects.toBeInstanceOf(IdentityChangedError);
  expect(publish).not.toHaveBeenCalled();
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(sessionStorage.length).toBe(0);
});

it('refuses every early publication when identity changes before the hierarchy finishes', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const original = h.client.readWorkspace.bind(h.client);
  vi.spyOn(h.client, 'readWorkspace').mockImplementation(async (...args) => {
    const workspace = await original(...args);
    h.currentUser.mockResolvedValue('bob');
    return workspace;
  });
  const publish = vi.fn();
  await expect(
    loadV2Agent(
      h.client,
      'agt_1',
      new AbortController().signal,
      session,
      false,
      publish,
      undefined,
      publish,
    ),
  ).rejects.toBeInstanceOf(IdentityChangedError);
  expect(publish).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
});

it('retains the fresh mutation barrier even while an entry handoff is available', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.currentUser.mockResolvedValue('bob');
  await expect(session.send('retained draft', vi.fn())).rejects.toBeInstanceOf(
    IdentityChangedError,
  );
  expect(h.post).not.toHaveBeenCalled();
});
