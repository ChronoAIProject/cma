import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { useOpeningPreparation } from '../useOpeningPreparation';
import { loadAgent } from '../loadAgent';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../../test/agent';
import { harness } from './controller.fixture';
import { AgentV2Session } from './client';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { prepareOpeningReads, takeOpeningReads } from './openingReads';

const lineage = {
  agentId: AGENT_DETAIL.id,
  workspaceId: AGENT_DETAIL.workspace.id,
  sandboxId: AGENT_DETAIL.sandbox.id,
};
const deferred = <Value,>() => {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
};

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function opening() {
  const h = harness();
  const eligible = await h.eligibility();
  const selection = deferred<Response>();
  h.eligibility.mockClear().mockReturnValue(selection.promise);
  const detail = vi.spyOn(h.client, 'readAgent');
  const transcript = vi.spyOn(h.client, 'readAgentTranscript');
  const usage = vi.spyOn(h.client, 'readAgentUsage');
  const view = renderHook(
    ({ authorized, refused }) =>
      useOpeningPreparation(h.client, 'agt_1', 'alice', authorized, refused, lineage),
    { initialProps: { authorized: false, refused: false } },
  );
  await waitFor(() => expect(h.eligibility).toHaveBeenCalledOnce());
  return { h, eligible, selection, detail, transcript, usage, view };
}

it('admits route projections after access and a fresh check while eligibility is pending', async () => {
  const o = await opening();
  expect(o.detail).not.toHaveBeenCalled();
  expect(o.transcript).not.toHaveBeenCalled();
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.transcript).toHaveBeenCalledOnce());
  expect(o.detail).toHaveBeenCalledOnce();
  expect(o.usage).toHaveBeenCalledOnce();
  expect(o.h.currentUser).toHaveBeenCalledTimes(2);
  expect(o.h.snapshot).not.toHaveBeenCalled();
  const input = vi.fn();
  const result = loadAgent(
    o.h.client,
    'agt_1',
    new AbortController().signal,
    false,
    undefined,
    lineage,
    input,
  );
  expect(input).not.toHaveBeenCalled();
  o.selection.resolve(o.eligible);
  // Supply a valid head so early publication and full completion are both covered.
  o.h.head.mockResolvedValue(
    new Response(
      JSON.stringify({
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: 'agt_1',
        incarnation: 'a'.repeat(64),
        through_cursor: `ag1.${'a'.repeat(64)}.0`,
        active_turn: null,
      }),
      { headers: { 'content-type': 'application/json' } },
    ),
  );
  await result;
  expect(o.detail).toHaveBeenCalledOnce();
  expect(o.transcript).toHaveBeenCalledOnce();
  expect(input).toHaveBeenCalled();
  expect(o.h.post).not.toHaveBeenCalled();
  o.view.unmount();
});

it('does not admit protected reads if the fresh account check refuses', async () => {
  const o = await opening();
  o.h.currentUser.mockResolvedValue('bob');
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.h.currentUser).toHaveBeenCalledTimes(2));
  expect(o.detail).not.toHaveBeenCalled();
  expect(o.transcript).not.toHaveBeenCalled();
  o.selection.resolve(o.eligible);
  await expect(
    loadAgent(o.h.client, 'agt_1', new AbortController().signal, false, undefined, lineage),
  ).rejects.toMatchObject({ name: 'IdentityChangedError' });
});

it.each(['bob', undefined])(
  'rechecks completed retained-v2 preparation after access when the account becomes %s',
  async (owner) => {
    const h = harness();
    vi.spyOn(performance, 'now').mockReturnValue(1000);
    const retained = vi.spyOn(AgentV2Session.prototype, 'retainEntryHandoff');
    agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
    const preparation = h.client.agentTransport!.prepareOpening('agt_1', 'alice');
    await waitFor(() => expect(retained).toHaveBeenCalledOnce());
    expect(h.currentUser).toHaveBeenCalledOnce();
    // Route access follows completed speculative selection, within its proof lifetime.
    h.currentUser.mockResolvedValue(owner);
    const detail = vi.spyOn(h.client, 'readAgent');
    const transcript = vi.spyOn(h.client, 'readAgentTranscript');
    const usage = vi.spyOn(h.client, 'readAgentUsage');
    const pending = prepareOpeningReads(h.client, 'agt_1', preparation);
    const taken = takeOpeningReads(h.client, 'agt_1', new AbortController().signal)!;
    try {
      await expect(taken.read()).rejects.toMatchObject({ name: 'IdentityChangedError' });
      expect(h.currentUser).toHaveBeenCalledTimes(2);
      expect(detail).not.toHaveBeenCalled();
      expect(transcript).not.toHaveBeenCalled();
      expect(usage).not.toHaveBeenCalled();
      expect(h.post).not.toHaveBeenCalled();
    } finally {
      taken.dispose();
      pending.release();
      preparation.release();
    }
  },
);

it.each(['refusal', 'unmount'] as const)('aborts unconsumed data on %s', async (reason) => {
  const o = await opening();
  const pending = deferred<typeof AGENT_TRANSCRIPT>();
  o.transcript.mockReturnValue(pending.promise);
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.transcript).toHaveBeenCalledOnce());
  const signal = o.transcript.mock.calls[0]![1]!.signal!;
  if (reason === 'refusal') o.view.rerender({ authorized: false, refused: true });
  else o.view.unmount();
  expect(signal.aborted).toBe(true);
  await act(async () => {
    pending.resolve(AGENT_TRANSCRIPT);
    o.selection.resolve(o.eligible);
  });
  expect(o.h.snapshot).not.toHaveBeenCalled();
  expect(o.h.post).not.toHaveBeenCalled();
});

it('refuses a completed preparation across the same account sign-in epoch', async () => {
  const o = await opening();
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.transcript).toHaveBeenCalledOnce());
  clearAgentTransport(sessionStorage);
  agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  o.selection.resolve(o.eligible);
  const publish = vi.fn();
  await expect(
    loadAgent(
      o.h.client,
      'agt_1',
      new AbortController().signal,
      false,
      undefined,
      lineage,
      publish,
    ),
  ).rejects.toMatchObject({ name: 'IdentityChangedError' });
  expect(publish).not.toHaveBeenCalled();
});

it('propagates a prepared usage failure without issuing a replacement read', async () => {
  const o = await opening();
  const failure = new ApiError(429, 'Busy', 'rate_limited', undefined, 45000);
  o.usage.mockRejectedValue(failure);
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.usage).toHaveBeenCalledOnce());
  o.selection.resolve(o.eligible);
  await expect(
    loadAgent(o.h.client, 'agt_1', new AbortController().signal, false, undefined, lineage),
  ).rejects.toBe(failure);
  expect(o.usage).toHaveBeenCalledOnce();
});

it('expires abandoned preparation and never offers it to a later navigation', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const transcript = vi.spyOn(h.client, 'readAgentTranscript');
  vi.useFakeTimers();
  prepareOpeningReads(
    h.client,
    'agt_1',
    {
      handoff: vi.fn(),
      release: vi.fn(),
      authorizeRead: async () => ({
        owner: session.owner,
        assertCurrent: () => {
          session.journal.read();
        },
      }),
    },
    lineage,
  );
  await vi.advanceTimersByTimeAsync(0);
  const signal = transcript.mock.calls[0]![1]!.signal!;
  await vi.advanceTimersByTimeAsync(5000);
  expect(signal.aborted).toBe(true);
  expect(
    takeOpeningReads(h.client, 'agt_1', new AbortController().signal, lineage),
  ).toBeUndefined();
});

it('rejects changed lineage and does not consume another client preparation', async () => {
  const o = await opening();
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.transcript).toHaveBeenCalledOnce());
  const signal = o.transcript.mock.calls[0]![1]!.signal!;
  expect(
    takeOpeningReads(harness().client, 'agt_1', new AbortController().signal, lineage),
  ).toBeUndefined();
  expect(signal.aborted).toBe(false);
  expect(
    takeOpeningReads(o.h.client, 'agt_1', new AbortController().signal, {
      ...lineage,
      workspaceId: 'different-workspace',
    }),
  ).toBeUndefined();
  expect(signal.aborted).toBe(true);
  o.view.unmount();
  o.selection.resolve(o.eligible);
});

it('transfers cancellation ownership without letting route cleanup abort the load', async () => {
  const o = await opening();
  o.view.rerender({ authorized: true, refused: false });
  await waitFor(() => expect(o.transcript).toHaveBeenCalledOnce());
  const request = new AbortController();
  const taken = takeOpeningReads(o.h.client, 'agt_1', request.signal, lineage)!;
  const reads = await taken.read();
  const signal = o.transcript.mock.calls[0]![1]!.signal!;
  // Once consumed, only the load's own cancellation/disposal controls this batch.
  o.view.unmount();
  expect(signal.aborted).toBe(false);
  request.abort();
  expect(signal.aborted).toBe(true);
  expect(reads.assertCurrent).toThrow();
  taken.dispose();
  o.selection.resolve(o.eligible);
});

it('shares an exact navigation projection batch with the activated route', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const transcript = vi.spyOn(h.client, 'readAgentTranscript');
  const preparation = {
    handoff: vi.fn(),
    release: vi.fn(),
    authorizeRead: async () => ({
      owner: session.owner,
      assertCurrent: () => session.journal.read(),
    }),
  };
  const first = prepareOpeningReads(h.client, 'agt_1', preparation, lineage);
  await waitFor(() => expect(transcript).toHaveBeenCalledOnce());
  const transcriptSignal = transcript.mock.calls[0]![1]!.signal!;
  const second = prepareOpeningReads(h.client, 'agt_1', preparation, lineage);
  expect(transcript).toHaveBeenCalledOnce();
  first.release();
  expect(transcriptSignal.aborted).toBe(false);
  second.release();
  expect(transcriptSignal.aborted).toBe(true);
});
