import { waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { harness } from './controller.fixture';
import { loadV2Agent } from './load';
import { clearAgentTransport, agentTransportJournal } from './journal';
import { loadAgent } from '../loadAgent';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it.each(['usage', 'snapshot'] as const)(
  'settles a failed %s load while publication authorization remains pending',
  async (source) => {
    const h = harness();
    const session = (await h.client.agentTransport!.open('agt_1'))!;
    const failed = deferred<never>();
    if (source === 'usage') vi.spyOn(h.client, 'readAgentUsage').mockReturnValue(failed.promise);
    else vi.spyOn(session.events, 'snapshot').mockReturnValue(failed.promise);
    const proof = deferred<string | undefined>();
    h.currentUser.mockResolvedValueOnce('alice').mockReturnValue(proof.promise);
    const publish = vi.fn();
    const loading = loadV2Agent(
      h.client,
      'agt_1',
      new AbortController().signal,
      session,
      true,
      undefined,
      undefined,
      publish,
    );
    await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(4));
    failed.reject(new Error('Parent projection failed'));
    await expect(loading).rejects.toThrow('Parent projection failed');
    expect(publish).not.toHaveBeenCalled();
    proof.reject(new Error('Late identity failure'));
    await Promise.resolve();
    expect(publish).not.toHaveBeenCalled();
  },
);

it('refuses a prepared opening after same-account sign-out/sign-in without touching its newer journal', async () => {
  const h = harness();
  const opening = h.client.agentTransport!.prepareOpening('agt_1', 'alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  clearAgentTransport(sessionStorage);
  const newer = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  newer.select(true);
  const pending = newer.retain('New sign-in request');
  opening.handoff();
  await expect(loadAgent(h.client, 'agt_1', new AbortController().signal)).rejects.toMatchObject({
    name: 'IdentityChangedError',
  });
  expect(newer.read()).toEqual(pending);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
  opening.release();
});
