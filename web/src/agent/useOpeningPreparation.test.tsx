import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { harness } from './transport/controller.fixture';
import { useOpeningPreparation } from './useOpeningPreparation';
import { loadAgent } from './loadAgent';
import { agentTransportJournal } from './transport/journal';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it('selects alongside access classification but reads no Agent data until authorized', async () => {
  const h = harness();
  const detail = vi.spyOn(h.client, 'readAgent');
  const view = renderHook(
    ({ authorized }) => useOpeningPreparation(h.client, 'agt_1', 'alice', authorized, false),
    { initialProps: { authorized: false } },
  );
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  expect(detail).not.toHaveBeenCalled();
  expect(h.snapshot).not.toHaveBeenCalled();
  view.rerender({ authorized: true });
  h.currentUser.mockResolvedValue('bob');
  await expect(loadAgent(h.client, 'agt_1', new AbortController().signal)).rejects.toMatchObject({
    name: 'IdentityChangedError',
  });
  expect(detail).not.toHaveBeenCalled();
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
});

it('reuses a transferred creation selection without restarting or replacing it', async () => {
  const h = harness();
  const preparation = h.client.agentTransport!.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  preparation.handoff();
  preparation.release();
  const view = renderHook(() => useOpeningPreparation(h.client, 'agt_1', 'alice', true, false));
  const result = await loadAgent(h.client, 'agt_1', new AbortController().signal);
  expect(result.v2?.owner).toBe('alice');
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(h.eligibility).toHaveBeenCalledOnce();
  view.unmount();
  expect(() => result.v2?.journal.read()).not.toThrow();
});

it.each(['refused', 'unmounted'] as const)(
  'cancels unconsumed selection when access is %s',
  async (reason) => {
    const h = harness();
    let resolve!: (owner: string) => void;
    h.currentUser.mockReturnValueOnce(new Promise<string>((done) => (resolve = done)));
    const view = renderHook(
      ({ refused }) => useOpeningPreparation(h.client, 'agt_1', 'alice', false, refused),
      { initialProps: { refused: false } },
    );
    await waitFor(() => expect(h.currentUser).toHaveBeenCalledOnce());
    const signal = (h.currentUser.mock.calls[0] as [AbortSignal?] | undefined)?.[0];
    if (reason === 'refused') view.rerender({ refused: true });
    else view.unmount();
    expect(signal?.aborted).toBe(true);
    resolve('alice');
    await Promise.resolve();
    await Promise.resolve();
    expect(h.eligibility).not.toHaveBeenCalled();
    expect(sessionStorage.length).toBe(0);
  },
);

it('cancels the prior route and only hands the selected Agent to its load', async () => {
  const h = harness();
  let resolve!: (owner: string) => void;
  h.currentUser.mockReturnValueOnce(new Promise<string>((done) => (resolve = done)));
  const view = renderHook(
    ({ agent, authorized }) => useOpeningPreparation(h.client, agent, 'alice', authorized, false),
    { initialProps: { agent: 'old-agent', authorized: false } },
  );
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledOnce());
  view.rerender({ agent: 'agt_1', authorized: true });
  resolve('alice');
  const result = await loadAgent(h.client, 'agt_1', new AbortController().signal);
  expect(result.detail.id).toBe('agt_1');
  expect(agentTransportJournal(sessionStorage, 'alice', 'old-agent').selected()).toBeUndefined();
  expect(h.eligibility).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
});

it('revokes a transferred selection when the route unmounts before consuming it', async () => {
  const h = harness();
  const existing = h.client.agentTransport!.prepare('agt_1', 'alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
  existing.handoff();
  existing.release();
  const view = renderHook(() => useOpeningPreparation(h.client, 'agt_1', 'alice', true, false));
  view.unmount();
  const result = await loadAgent(h.client, 'agt_1', new AbortController().signal);
  expect(result.v2?.owner).toBe('alice');
  // A new open has both selection checks, a load check and final publication.
  expect(h.currentUser).toHaveBeenCalledTimes(6);
  expect(h.eligibility).toHaveBeenCalledOnce();
});

it('cancels stale A to B to A selections without aborting the final load', async () => {
  const h = harness();
  let resolve!: (owner: string) => void;
  h.currentUser.mockReturnValueOnce(new Promise<string>((done) => (resolve = done)));
  const view = renderHook(
    ({ agent }) => useOpeningPreparation(h.client, agent, 'alice', true, false),
    { initialProps: { agent: 'agt_1' } },
  );
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledOnce());
  const signal = (h.currentUser.mock.calls[0] as [AbortSignal?])[0];
  view.rerender({ agent: 'agt_2' });
  view.rerender({ agent: 'agt_1' });
  expect(signal?.aborted).toBe(true);
  resolve('alice');
  const ready = await loadAgent(h.client, 'agt_1', new AbortController().signal);
  expect(ready.detail.id).toBe('agt_1');
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_2').selected()).toBeUndefined();
  expect(h.eligibility).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});
