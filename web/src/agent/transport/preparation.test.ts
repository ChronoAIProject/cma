import { waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { loadAgent } from '../loadAgent';
import { harness } from './controller.fixture';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { IdentityChangedError } from './identity';
import { createAgentTransport } from './client';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
}

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it('hands the pending exact-Agent selection to one load, which still authorizes before reading', async () => {
  const h = harness();
  const flag = deferred<Response>();
  const response = await h.eligibility();
  h.eligibility.mockClear();
  h.eligibility.mockReturnValueOnce(flag.promise);
  const transport = h.client.agentTransport!;
  const preparation = transport.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.eligibility).toHaveBeenCalledOnce());
  preparation.handoff();
  preparation.release();
  const detail = vi.spyOn(h.client, 'readAgent');
  const loading = loadAgent(h.client, 'agt_1', new AbortController().signal);
  expect(detail).not.toHaveBeenCalled();
  const fresh = deferred<string | undefined>();
  h.currentUser.mockResolvedValueOnce('alice').mockReturnValueOnce(fresh.promise);
  flag.resolve(response);
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(3));
  expect(detail).not.toHaveBeenCalled();
  expect(h.snapshot).not.toHaveBeenCalled();
  fresh.resolve('alice');
  expect(await loading).toMatchObject({ detail: { id: 'agt_1' }, v2: expect.anything() });
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(h.eligibility).toHaveBeenCalledOnce();
  await transport.openForLoad('agt_1');
  expect(h.currentUser).toHaveBeenCalledTimes(6);
  expect(h.eligibility).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
});

it.each(['bob', undefined])(
  'refuses the wrong initial owner %s before touching storage',
  async (owner) => {
    const h = harness();
    h.currentUser.mockResolvedValue(owner);
    h.client.agentTransport!.prepare('agt_1', 'alice').handoff();
    await expect(h.client.agentTransport!.openForLoad('agt_1')).rejects.toBeInstanceOf(
      IdentityChangedError,
    );
    expect(sessionStorage.length).toBe(0);
    expect(h.eligibility).not.toHaveBeenCalled();
  },
);

it.each(['different owner', 'new epoch'] as const)(
  'does not let the first account lookup replace a newer %s journal',
  async (change) => {
    const h = harness();
    const current = deferred<string | undefined>();
    h.currentUser.mockReturnValueOnce(current.promise);
    const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
    const newer = agentTransportJournal(
      sessionStorage,
      change === 'new epoch' ? 'alice' : 'bob',
      'agt_1',
    );
    newer.select(true);
    const pending = newer.retain('Keep the newer request');
    preparation.handoff();
    const loading = h.client.agentTransport!.openForLoad('agt_1');
    const rejected = expect(loading).rejects.toBeInstanceOf(IdentityChangedError);
    current.resolve('alice');
    await rejected;
    expect(newer.read()).toEqual(pending);
    expect(h.eligibility).not.toHaveBeenCalled();
  },
);

it('rejects an owner change during eligibility without selecting or reading the Agent', async () => {
  const h = harness();
  h.currentUser.mockResolvedValueOnce('alice').mockResolvedValue('bob');
  h.client.agentTransport!.prepare('agt_1', 'alice').handoff();
  await expect(loadAgent(h.client, 'agt_1', new AbortController().signal)).rejects.toBeInstanceOf(
    IdentityChangedError,
  );
  expect(sessionStorage.length).toBe(0);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
});

it.each(['bob', undefined])(
  'retains fresh load-entry refusal after prepared selection when owner becomes %s',
  async (owner) => {
    const h = harness();
    const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
    await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
    h.currentUser.mockResolvedValue(owner);
    preparation.handoff();
    const detail = vi.spyOn(h.client, 'readAgent');
    await expect(loadAgent(h.client, 'agt_1', new AbortController().signal)).rejects.toBeInstanceOf(
      IdentityChangedError,
    );
    expect(detail).not.toHaveBeenCalled();
    expect(h.snapshot).not.toHaveBeenCalled();
  },
);

it('keeps a newer journal intact when a transferred old session is loaded', async () => {
  const h = harness();
  const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  clearAgentTransport(sessionStorage);
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  const pending = journal.retain('New epoch');
  preparation.handoff();
  await expect(loadAgent(h.client, 'agt_1', new AbortController().signal)).rejects.toBeInstanceOf(
    IdentityChangedError,
  );
  expect(journal.read()).toEqual(pending);
  expect(h.snapshot).not.toHaveBeenCalled();
});

it('cancels ordinary modal cleanup before a delayed initial account result can write', async () => {
  const h = harness();
  const current = deferred<string | undefined>();
  h.currentUser.mockReturnValueOnce(current.promise);
  const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
  preparation.release();
  current.resolve('alice');
  await Promise.resolve();
  await Promise.resolve();
  expect(sessionStorage.length).toBe(0);
  expect(h.eligibility).not.toHaveBeenCalled();
  expect(await h.client.agentTransport!.openForLoad('agt_1')).toBeDefined();
  expect(h.currentUser).toHaveBeenCalledTimes(3);
});

it('cancels a transferred in-flight selection when its load is cancelled', async () => {
  const h = harness();
  const flag = deferred<Response>();
  const response = await h.eligibility();
  h.eligibility.mockClear();
  h.eligibility.mockReturnValueOnce(flag.promise);
  const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.eligibility).toHaveBeenCalledOnce());
  preparation.handoff();
  const request = new AbortController();
  const loading = loadAgent(h.client, 'agt_1', request.signal);
  const rejected = expect(loading).rejects.toMatchObject({ name: 'AbortError' });
  request.abort();
  flag.resolve(response);
  await rejected;
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').selected()).toBeUndefined();
  expect(h.snapshot).not.toHaveBeenCalled();
});

it('supersedes old preparation without letting a late owner lookup erase the new selection', async () => {
  const h = harness();
  const first = deferred<string | undefined>();
  h.currentUser.mockReturnValueOnce(first.promise);
  const old = h.client.agentTransport!.prepare('old-agent', 'alice');
  const next = h.client.agentTransport!.prepare('agt_1', 'alice');
  next.handoff();
  const loaded = await h.client.agentTransport!.openForLoad('agt_1');
  old.handoff();
  old.release();
  first.resolve('alice');
  await Promise.resolve();
  expect(loaded?.journal.selected()).toBe('responses_v2');
  expect(agentTransportJournal(sessionStorage, 'alice', 'old-agent').selected()).toBeUndefined();
  expect(h.eligibility).toHaveBeenCalledOnce();
});

it('never hands a prepared session to a different Agent', async () => {
  const h = harness();
  const preparation = h.client.agentTransport!.prepare('other-agent', 'alice');
  preparation.handoff();
  const result = await h.client.agentTransport!.openForLoad('agt_1');
  expect(result?.journal.agentId).toBe('agt_1');
  expect(agentTransportJournal(sessionStorage, 'alice', 'other-agent').selected()).toBeUndefined();
});

it('does not consume an untransferred preparation during an ordinary open', async () => {
  const h = harness();
  h.client.agentTransport!.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  await h.client.agentTransport!.openForLoad('agt_1');
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(h.eligibility).toHaveBeenCalledOnce();
});

it.each(['agui', 'responses_v2'] as const)(
  'preserves retained %s selection and request bytes',
  async (transport) => {
    const h = harness();
    const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
    journal.select(transport === 'responses_v2');
    if (transport === 'responses_v2') journal.retain('Keep original key and body');
    const before = { ...sessionStorage };
    h.eligibility.mockRejectedValue(new Error('Flag changed or unavailable'));
    h.client.agentTransport!.prepare('agt_1', 'alice').handoff();
    const result = await h.client.agentTransport!.openForLoad('agt_1');
    expect(result !== undefined).toBe(transport === 'responses_v2');
    expect({ ...sessionStorage }).toEqual(before);
    expect(h.eligibility).not.toHaveBeenCalled();
    expect(h.currentUser).toHaveBeenCalledTimes(transport === 'responses_v2' ? 2 : 4);
  },
);

it('uses fresh ordinary authentication for a new AGUI selection after flag outage', async () => {
  const h = harness();
  h.eligibility.mockRejectedValue(new Error('Discovery unavailable'));
  h.client.agentTransport!.prepare('agt_1', 'alice').handoff();
  expect(await h.client.agentTransport!.openForLoad('agt_1')).toBeUndefined();
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(h.eligibility).toHaveBeenCalledOnce();
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').selected()).toBe('agui');
});

it('does not replace the winner when another open selects AGUI during preparation', async () => {
  const h = harness();
  const flag = deferred<Response>();
  const response = await h.eligibility();
  h.eligibility.mockClear();
  h.eligibility.mockReturnValueOnce(flag.promise).mockRejectedValueOnce(new Error('Unavailable'));
  const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.eligibility).toHaveBeenCalledOnce());
  expect(await h.client.agentTransport!.open('agt_1')).toBeUndefined();
  preparation.handoff();
  flag.resolve(response);
  expect(await h.client.agentTransport!.openForLoad('agt_1')).toBeUndefined();
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').selected()).toBe('agui');
});

it.each(['corrupt', 'full'] as const)(
  'refuses %s storage without a fallback or projection reads',
  async (failure) => {
    const h = harness();
    if (failure === 'corrupt') sessionStorage.setItem('cma:agent-transport:v1:identity', '{');
    const storage: Storage = {
      getItem: (key) => sessionStorage.getItem(key),
      setItem: (key, value) => {
        if (failure === 'full') throw new Error('Storage full');
        sessionStorage.setItem(key, value);
      },
      removeItem: (key) => sessionStorage.removeItem(key),
      key: (index) => sessionStorage.key(index),
      clear: () => sessionStorage.clear(),
      get length() {
        return sessionStorage.length;
      },
    };
    const client = {
      ...h.client,
      agentTransport: createAgentTransport(h.sdk, { storage, currentUser: h.currentUser }),
    };
    client.agentTransport.prepare('agt_1', 'alice').handoff();
    await expect(loadAgent(client, 'agt_1', new AbortController().signal)).rejects.toBeInstanceOf(
      Error,
    );
    expect(h.eligibility).not.toHaveBeenCalled();
    expect(h.snapshot).not.toHaveBeenCalled();
    expect(h.post).not.toHaveBeenCalled();
  },
);
