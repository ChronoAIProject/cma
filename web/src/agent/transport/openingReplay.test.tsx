import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useOpeningPreparation } from '../useOpeningPreparation';
import { loadAgent } from '../loadAgent';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE } from '../../test/agent';
import { cursor, harness, incarnation, item, json, page } from './controller.fixture';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { prepareOpeningReads, takeOpeningReads } from './openingReads';

const lineage = {
  agentId: AGENT_DETAIL.id,
  workspaceId: AGENT_DETAIL.workspace.id,
  sandboxId: AGENT_DETAIL.sandbox.id,
};
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
};

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

async function opening(configure: (h: ReturnType<typeof harness>) => void = () => undefined) {
  const h = harness();
  h.head.mockImplementation(async () =>
    json({
      object: 'cma.agent.event_head',
      schema_version: 1,
      agent_id: 'agt_1',
      incarnation,
      through_cursor: cursor(0),
      active_turn: null,
    }),
  );
  configure(h);
  const view = renderHook(() =>
    useOpeningPreparation(h.client, 'agt_1', 'alice', true, false, lineage),
  );
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  await waitFor(() => expect(h.head).toHaveBeenCalledOnce());
  return { h, view };
}

it('overlaps prepared replay with load admission and consumes each read only once', async () => {
  const { h, view } = await opening();
  const admission = deferred<string | undefined>();
  const final = deferred<string | undefined>();
  h.currentUser
    .mockClear()
    .mockReturnValueOnce(admission.promise)
    .mockResolvedValueOnce('alice')
    .mockReturnValueOnce(final.promise);
  const publish = vi.fn();
  const settled = vi.fn();
  const loading = loadAgent(
    h.client,
    'agt_1',
    new AbortController().signal,
    false,
    undefined,
    lineage,
    publish,
  ).then((ready) => {
    settled();
    return ready;
  });
  await Promise.resolve();
  expect(publish).not.toHaveBeenCalled();
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.head).toHaveBeenCalledOnce();
  admission.resolve('alice');
  await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(3));
  expect(publish).not.toHaveBeenCalled();
  expect(settled).not.toHaveBeenCalled();
  final.resolve('alice');
  expect(await loading).toMatchObject({ historyLoading: false, historyUnavailable: false });
  // No transient history-loading notice when the prepared history is complete.
  expect(publish).not.toHaveBeenCalled();
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.head).toHaveBeenCalledOnce();
  // Entry, one completed projection batch, then fresh final completion.
  expect(h.currentUser).toHaveBeenCalledTimes(3);
  expect(
    new URL(h.snapshot.mock.calls[0]![0], 'https://fixture.invalid').searchParams.get('limit'),
  ).toBe('200');
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});

it.each(['history', 'usage', 'head'] as const)(
  'keeps early input publication while %s is pending',
  async (field) => {
    const pending = deferred<void>();
    const { h, view } = await opening((h) => {
      if (field === 'history')
        h.snapshot.mockImplementation(async () => {
          await pending.promise;
          return json(page([], 0));
        });
      else if (field === 'usage')
        vi.spyOn(h.client, 'readAgentUsage').mockImplementation(async () => {
          await pending.promise;
          return AGENT_USAGE;
        });
      else {
        const readHead = h.head.getMockImplementation()!;
        h.head.mockImplementation(async () => {
          await pending.promise;
          return readHead();
        });
      }
    });
    const publish = vi.fn();
    const loading = loadAgent(
      h.client,
      'agt_1',
      new AbortController().signal,
      false,
      undefined,
      lineage,
      publish,
    );
    await waitFor(() => expect(publish).toHaveBeenCalled());
    expect(publish.mock.calls[0]![0].historyLoading).toBe(true);
    pending.resolve();
    expect(await loading).toMatchObject({ historyLoading: false, historyUnavailable: false });
    expect(h.snapshot).toHaveBeenCalledOnce();
    view.unmount();
  },
);

it.each(['bob', undefined])(
  'refuses prefetched results when final authority becomes %s',
  async (owner) => {
    const { h, view } = await opening();
    // Entry succeeds; the final post-projection check must still be a new request.
    h.currentUser.mockResolvedValueOnce('alice').mockResolvedValue(owner);
    await expect(
      loadAgent(h.client, 'agt_1', new AbortController().signal, false, undefined, lineage),
    ).rejects.toMatchObject({ name: 'IdentityChangedError' });
    expect(h.snapshot).toHaveBeenCalledOnce();
    expect(h.post).not.toHaveBeenCalled();
    view.unmount();
  },
);

it('refuses completed replay across a same-account sign-in epoch change', async () => {
  const { h, view } = await opening();
  clearAgentTransport(sessionStorage);
  const newer = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  newer.select(true);
  const retained = newer.retain('New sign-in request');
  const publish = vi.fn();
  await expect(
    loadAgent(h.client, 'agt_1', new AbortController().signal, false, undefined, lineage, publish),
  ).rejects.toMatchObject({ name: 'IdentityChangedError' });
  expect(publish).not.toHaveBeenCalled();
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(newer.read()).toEqual(retained);
  view.unmount();
});

it('transfers replay cancellation to the load and rejects later use', async () => {
  const pending = deferred<void>();
  const { h, view } = await opening((h) => {
    const readHead = h.head.getMockImplementation()!;
    h.head.mockImplementation(async () => {
      await pending.promise;
      return readHead();
    });
    h.snapshot.mockImplementation(async () => {
      await pending.promise;
      return json(page([], 0));
    });
  });
  const request = new AbortController();
  const taken = takeOpeningReads(h.client, 'agt_1', request.signal, lineage)!;
  const reads = await taken.read();
  const signals = h.fetch.mock.calls
    .filter(([url]) => /events\/(snapshot|head)/.test(String(url)))
    .map(([, init]) => init!.signal!);
  expect(signals).toHaveLength(2);
  view.unmount();
  expect(signals.every((signal) => !signal.aborted)).toBe(true);
  request.abort();
  expect(signals.every((signal) => signal.aborted)).toBe(true);
  expect(reads.assertCurrent).toThrow();
  taken.dispose();
  pending.resolve();
  await Promise.allSettled([reads.replay!.firstPage, reads.replay!.head]);
  expect(h.snapshot).toHaveBeenCalledOnce();
  expect(h.post).not.toHaveBeenCalled();
});

it.each([403, 503])(
  'retains a prepared HTTP %s failure without a replacement page',
  async (status) => {
    const { h, view } = await opening((h) =>
      h.snapshot.mockImplementation(async () =>
        json(
          {
            error: {
              type: 'server_error',
              code: 'agent_read_unavailable',
              message: 'Read refused by fixture',
              param: null,
            },
          },
          status,
        ),
      ),
    );
    const loading = loadAgent(
      h.client,
      'agt_1',
      new AbortController().signal,
      true,
      undefined,
      lineage,
    );
    if (status === 403) await expect(loading).rejects.toMatchObject({ status: 403 });
    else expect(await loading).toMatchObject({ historyLoading: false, historyUnavailable: true });
    expect(h.snapshot).toHaveBeenCalledOnce();
    view.unmount();
  },
);

it('does not start replay when the exact selected transport is legacy', async () => {
  const h = harness();
  agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(false);
  const preparation = h.client.agentTransport!.prepareOpening('agt_1', 'alice');
  const lease = prepareOpeningReads(h.client, 'agt_1', preparation, lineage);
  const taken = takeOpeningReads(h.client, 'agt_1', new AbortController().signal, lineage)!;
  try {
    const reads = await taken.read();
    expect(await reads.replay?.firstPage).toBeUndefined();
    expect(await reads.replay?.head).toBeUndefined();
    expect(h.snapshot).not.toHaveBeenCalled();
    expect(h.head).not.toHaveBeenCalled();
  } finally {
    taken.dispose();
    lease.release();
    preparation.release();
  }
});

it('recaptures a newer fixed prefix when transcript outgrows the prepared page', async () => {
  const { h, view } = await opening((h) => {
    vi.mocked(h.client.readAgentTranscript).mockResolvedValue(AGENT_TRANSCRIPT);
    h.snapshot
      .mockResolvedValueOnce(json(page([], 0)))
      .mockImplementation(async () =>
        json(
          page(
            [
              item(1, { type: 'turn_started', turn_sequence: 1 }),
              item(2, { type: 'message', role: 'user', content: 'Inspect this repository.' }),
              item(3, { type: 'message', role: 'assistant', content: 'The repository is ready.' }),
              item(4, { type: 'turn_completed', turn_sequence: 1 }),
            ],
            4,
          ),
        ),
      );
  });
  const result = await loadAgent(
    h.client,
    'agt_1',
    new AbortController().signal,
    false,
    undefined,
    lineage,
  );
  expect(result).toMatchObject({ historyUnavailable: false, conversation: { cursor: 4 } });
  expect(result.v2Items).toHaveLength(4);
  expect(h.snapshot).toHaveBeenCalledTimes(2);
  for (const [url] of h.snapshot.mock.calls)
    expect(new URL(url, 'https://fixture.invalid').searchParams.get('limit')).toBe('200');
  view.unmount();
});
