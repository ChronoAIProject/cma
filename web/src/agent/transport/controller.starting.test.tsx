import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL, agentClient } from '../../test/agent';
import { sandboxFixture } from '../../test/hierarchy';
import { BOOTSTRAP_AGENT, bootstrapSandbox } from '../../test/bootstrap';
import type { SandboxDetail } from '../../api/types';
import { useAgentController } from '../controller';
import { agentTransportJournal } from './journal';
import { accepted, harness } from './controller.fixture';

const starting = {
  ...AGENT_DETAIL,
  status: 'starting' as const,
  firstMessage: { state: 'queued' as const, queuedAt: AGENT_DETAIL.createdAt },
};
const parent = sandboxFixture();
const preparing: SandboxDetail = {
  ...parent,
  lifecycle: { ...parent.lifecycle!, pause: { allowed: false, reason: 'work_in_progress' } },
};
beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it.each(['provisioning', 'installing'] as const)(
  'durably queues bootstrap input while the parent is %s without claiming runtime readiness',
  async (status) => {
    const h = harness();
    const detail = vi.spyOn(h.client, 'readAgent').mockResolvedValue(BOOTSTRAP_AGENT);
    vi.spyOn(h.client, 'readSandbox').mockResolvedValue(bootstrapSandbox(status));
    const view = renderHook(() => useAgentController(h.client, 'agt_1', { viewerId: 'alice' }));
    await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
    expect(detail.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(view.result.current.resourceAvailability.state).toBe('inactive');
    const accepted = vi.fn();
    await act(() => view.result.current.send('Queue after the original first message', accepted));
    expect(h.post).toHaveBeenCalledOnce();
    expect(accepted).toHaveBeenCalledOnce();
    expect(view.result.current.state).toMatchObject({
      detail: { status: 'starting', firstMessage: { state: 'queued' } },
    });
    expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').read()).toMatchObject({
      state: 'accepted',
      input: 'Queue after the original first message',
    });
    expect(h.client.runAgent).not.toHaveBeenCalled();
    view.unmount();
  },
);

it('invalidates a captured bootstrap send when current lineage proof is withdrawn', async () => {
  vi.useFakeTimers();
  const h = harness();
  const detail = vi.spyOn(h.client, 'readAgent').mockResolvedValue(BOOTSTRAP_AGENT);
  vi.spyOn(h.client, 'readSandbox').mockResolvedValue(bootstrapSandbox());
  const view = renderHook(() => useAgentController(h.client, 'agt_1', { viewerId: 'alice' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  const oldSend = view.result.current.send;
  detail.mockResolvedValue({ ...BOOTSTRAP_AGENT, bootstrap: undefined });
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(() => oldSend('Do not send from stale bootstrap proof'));
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});

it('holds bootstrap input on AG-UI even with valid server lineage', async () => {
  const runAgent = vi.fn();
  const client = agentClient({
    readAgent: async () => BOOTSTRAP_AGENT,
    readSandbox: async () => bootstrapSandbox(),
    runAgent,
  });
  const view = renderHook(() => useAgentController(client, 'agt_1', { viewerId: 'alice' }));
  await waitFor(() => expect(view.result.current.progressObservation).toBeDefined());
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(() => view.result.current.send('Do not submit through AG-UI'));
  expect(runAgent).not.toHaveBeenCalled();
  view.unmount();
});

it('accepts a Starting message only through the durable v2 receipt while retaining observation', async () => {
  const h = harness();
  vi.spyOn(h.client, 'readAgent').mockResolvedValue(starting);
  vi.spyOn(h.client, 'readSandbox').mockResolvedValue(preparing);
  let resolve!: (response: Response) => void;
  h.post.mockReturnValueOnce(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() =>
    expect(h.streaming && view.result.current.inputAvailability.allowed).toBe(true),
  );
  const onAccepted = vi.fn();
  let sent!: Promise<void>;
  act(() => {
    sent = view.result.current.send('Follow the repository setup', onAccepted);
  });
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  const saved = agentTransportJournal(sessionStorage, 'alice', 'agt_1').read();
  expect(saved).toMatchObject({ state: 'pending', input: 'Follow the repository setup' });
  expect(new Headers(h.post.mock.calls[0]![0]!.headers).get('idempotency-key')).toBe(saved?.key);
  expect(onAccepted).not.toHaveBeenCalled();
  expect(h.cancel).not.toHaveBeenCalled();
  await act(async () => {
    resolve(accepted());
    await sent;
  });
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(view.result.current.state).toMatchObject({
    detail: { status: 'starting', firstMessage: { state: 'queued' } },
  });
  expect(saved?.key).toBe(agentTransportJournal(sessionStorage, 'alice', 'agt_1').read()?.key);
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').read()?.state).toBe('accepted');
  expect(h.client.runAgent).not.toHaveBeenCalled();
  view.unmount();
});

it('retains an uncertain Starting request across Ready and reload, then explicitly recovers the exact body/key', async () => {
  const h = harness();
  const detail = vi.spyOn(h.client, 'readAgent').mockResolvedValue(starting);
  h.post.mockRejectedValueOnce(new TypeError('connection lost'));
  const first = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(first.result.current.inputAvailability.allowed).toBe(true));
  await act(() => first.result.current.send('Exact Starting input'));
  expect(first.result.current.pendingMessage).toBe('Exact Starting input');
  first.unmount();
  detail.mockResolvedValue(AGENT_DETAIL);
  const second = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(second.result.current.inputAvailability.allowed).toBe(true));
  expect(second.result.current.pendingMessage).toBe('Exact Starting input');
  expect(h.post).toHaveBeenCalledOnce();
  await act(() => second.result.current.recoverPending!());
  expect(h.post).toHaveBeenCalledTimes(2);
  const [before, after] = h.post.mock.calls.map(([init]) => init!);
  expect(after!.body).toBe(before!.body);
  expect(new Headers(after!.headers).get('idempotency-key')).toBe(
    new Headers(before!.headers).get('idempotency-key'),
  );
  expect(h.client.runAgent).not.toHaveBeenCalled();
  second.unmount();
});

it.each([
  [
    'checkpoint',
    {
      ...parent,
      lifecycle: {
        ...parent.lifecycle!,
        checkpoint: { phase: 'capture', startedAt: AGENT_DETAIL.createdAt, epoch: 1 },
      },
    },
  ],
  [
    'operation',
    {
      ...parent,
      lifecycle: {
        ...parent.lifecycle!,
        operation: { purpose: 'pause', phase: 'pause_agents', startedAt: AGENT_DETAIL.createdAt },
      },
    },
  ],
  ['paused', sandboxFixture('paused')],
  ['missing lifecycle', { ...parent, lifecycle: undefined }],
  [
    'checkpoint eligibility',
    {
      ...parent,
      lifecycle: {
        ...parent.lifecycle!,
        pause: { allowed: false, reason: 'checkpoint_in_progress' },
      },
    },
  ],
] as const)('refuses Starting input with %s even through a direct send', async (_name, sandbox) => {
  const h = harness();
  vi.spyOn(h.client, 'readAgent').mockResolvedValue(starting);
  vi.spyOn(h.client, 'readSandbox').mockResolvedValue(sandbox);
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(view.result.current.progressObservation).toBeDefined());
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(() => view.result.current.send('Hold this message'));
  expect(h.post).not.toHaveBeenCalled();
  expect(h.client.runAgent).not.toHaveBeenCalled();
  view.unmount();
});

it('invalidates a captured early-send callback after a parent observation becomes stale', async () => {
  vi.useFakeTimers();
  const h = harness();
  vi.spyOn(h.client, 'readAgent').mockResolvedValue(starting);
  vi.spyOn(h.client, 'readSandbox')
    .mockResolvedValueOnce(preparing)
    .mockRejectedValue(new Error('offline'));
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  const capturedSend = view.result.current.send;
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(() => capturedSend('Stale callback'));
  await act(() => view.result.current.send('Current callback'));
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});

it('refuses direct early sends on existing AG-UI transport', async () => {
  const runAgent = vi.fn();
  const client = agentClient({ readAgent: async () => starting, runAgent });
  const view = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  await act(() => view.result.current.send('Do not use unjournaled AG-UI'));
  expect(runAgent).not.toHaveBeenCalled();
  view.unmount();
});

it('holds uncertain Starting recovery while the parent is paused and preserves the original request', async () => {
  vi.useFakeTimers();
  const h = harness();
  vi.spyOn(h.client, 'readAgent').mockResolvedValue(starting);
  const read = vi.spyOn(h.client, 'readSandbox').mockResolvedValue(parent);
  h.post.mockRejectedValueOnce(new TypeError('connection lost'));
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  await act(() => view.result.current.send('Keep this Starting request'));
  expect(h.post).toHaveBeenCalledOnce();
  const saved = agentTransportJournal(sessionStorage, 'alice', 'agt_1').read();
  read.mockResolvedValue(sandboxFixture('paused'));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  await act(() => view.result.current.recoverPending!());
  expect(h.post).toHaveBeenCalledOnce();
  expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').read()).toEqual(saved);
  read.mockResolvedValue(parent);
  await act(() => vi.advanceTimersByTimeAsync(5000));
  await act(() => view.result.current.recoverPending!());
  expect(h.post).toHaveBeenCalledTimes(2);
  const [before, after] = h.post.mock.calls.map(([init]) => init!);
  expect(after!.body).toBe(before!.body);
  expect(new Headers(after!.headers).get('idempotency-key')).toBe(saved?.key);
  view.unmount();
});
