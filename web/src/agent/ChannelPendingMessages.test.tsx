import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import {
  ApiError,
  createClient,
  type AgentProvenance,
  type BridgePartition,
  type PendingChannelMessages,
} from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import type { BridgeViewsClient } from '../bridges/client';
import { ChannelPendingMessages } from './ChannelPendingMessages';
import type { ProvenanceState } from './provenance';

const logicalPartition = '812fafd5319325208bfad08915764686504091acfa9021826cad8fe921541a3c';
const allocation = 'bral_ff9c553b3044378f172040c31ecd7ed5fbed2a8b67095fce2844fd207eeeba8e';
const partitionRow = (): BridgePartition => ({
  id: allocation,
  bridge_id: 'brg_one',
  generation: 1,
  revision: 1,
  config_revision: 1,
  current: true,
  state: 'ready',
  placement: 'new_sandbox',
  context_sharing: 'chat_sender',
  target: {
    status: 'available',
    ids: {
      sandbox_id: 'sbx_one',
      workspace_id: 'wks_one',
      worktree_id: 'wkt_one',
      agent_id: 'agt_one',
    },
    present: ['sandbox', 'workspace', 'worktree', 'agent'],
  },
  source: { status: 'unavailable' },
  observed_at_ms: 1789700660000,
});
const value = (): PendingChannelMessages => ({
  status: 'available',
  bridge_id: 'brg_one',
  partition_id: allocation,
  generation: 1,
  agent_id: 'agt_one',
  through_sequence: 20,
  messages: [
    {
      command_id: 'cmd_one',
      response_id: 'resp_one',
      agent_order: 1,
      admitted_at: '2026-09-18T03:04:20Z',
      platform: 'telegram',
      text: 'Queued from Telegram',
      text_truncated: false,
    },
  ],
  pending_truncated: false,
  source_unavailable: false,
  observed_at_ms: 1789700660000,
});
const facts: AgentProvenance = {
  agent_id: 'agt_one',
  created_at: '2026-09-18T03:00:00Z',
  creation_origin: 'channel_bridge',
  details: {
    created_by: null,
    trigger: null,
    workspace_profile_index: null,
    channel_bridge: {
      bridge_id: 'brg_one',
      partition_id: logicalPartition,
      generation: 1,
      source_reference: 'source',
    },
  },
};
const provenance: ProvenanceState = { status: 'ready', agentId: 'agt_one', value: facts };
const flush = () => act(async () => undefined);
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fixture() {
  const pendingMessages = vi.fn().mockResolvedValue(value());
  const listPartitions = vi.fn().mockResolvedValue({ items: [partitionRow()], next_cursor: null });
  const client = agentClient({
    bridgeViews: { bridges: { pendingMessages, listPartitions } } as unknown as BridgeViewsClient,
  });
  const draw = (overrides: Partial<Parameters<typeof ChannelPendingMessages>[0]> = {}) => (
    <CmaClientProvider client={client}>
      <ChannelPendingMessages
        agentId="agt_one"
        viewerId="owner"
        provenance={provenance}
        cursor={20}
        enabled
        {...overrides}
      />
    </CmaClientProvider>
  );
  return { pendingMessages, listPartitions, draw };
}

it('shows accepted text before a turn, polls without reload, and invalidates it when replay advances', async () => {
  const { pendingMessages, listPartitions, draw } = fixture();
  const view = render(draw());
  await flush();
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
  expect(screen.getByText('Telegram message received · Waiting to start')).toBeInTheDocument();
  expect(pendingMessages).toHaveBeenCalledWith('brg_one', allocation, {
    signal: expect.any(AbortSignal),
  });
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(pendingMessages).toHaveBeenCalledTimes(2);
  expect(listPartitions).toHaveBeenCalledOnce();
  let finish!: (result: PendingChannelMessages) => void;
  pendingMessages.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  view.rerender(draw({ cursor: 22 }));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  await flush();
  expect(pendingMessages).toHaveBeenCalledTimes(3);
  await act(async () => finish(value()));
  // A stale in-flight success cannot duplicate the new durable owner message.
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  pendingMessages.mockResolvedValue({ ...value(), through_sequence: 22, messages: [] });
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
});

it('does not fetch private messages for shared readers, missing provenance or manual Agents', async () => {
  const { pendingMessages, draw } = fixture();
  const view = render(
    draw({
      provenance: { ...provenance, value: { ...facts, details: undefined } } as ProvenanceState,
    }),
  );
  await flush();
  view.rerender(
    draw({
      provenance: {
        status: 'ready',
        agentId: 'agt_one',
        value: { ...facts, creation_origin: 'manual' },
      },
    }),
  );
  await flush();
  view.rerender(draw({ enabled: false }));
  await flush();
  expect(pendingMessages).not.toHaveBeenCalled();
});

it('clears private previews on an authorization failure or redaction', async () => {
  const { pendingMessages, draw } = fixture();
  render(draw());
  await flush();
  pendingMessages.mockRejectedValueOnce(new Error('Access revoked'));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  pendingMessages.mockResolvedValue({ status: 'redacted' });
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
});

it('retries snapshot conflicts after 250ms twice, then backs off until success resets the limit', async () => {
  const { pendingMessages, listPartitions, draw } = fixture();
  render(draw());
  await flush();
  pendingMessages.mockRejectedValue(new ApiError(409, 'Snapshot changed'));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  expect(pendingMessages).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(249));
  expect(pendingMessages).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(pendingMessages).toHaveBeenCalledTimes(3);
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(pendingMessages).toHaveBeenCalledTimes(4);
  await act(() => vi.advanceTimersByTimeAsync(1999));
  expect(pendingMessages).toHaveBeenCalledTimes(4);
  pendingMessages.mockResolvedValue(value());
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
  pendingMessages.mockRejectedValueOnce(new ApiError(409, 'Snapshot changed again'));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(pendingMessages).toHaveBeenCalledTimes(6);
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(pendingMessages).toHaveBeenCalledTimes(7);
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
  expect(listPartitions).toHaveBeenCalledOnce();
});

it('keeps private text cleared and uses the full authorization backoff after a snapshot conflict', async () => {
  const { pendingMessages, draw } = fixture();
  render(draw());
  await flush();
  pendingMessages.mockRejectedValueOnce(new ApiError(409, 'Snapshot changed'));
  pendingMessages.mockRejectedValue(new ApiError(403, 'Access removed'));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(pendingMessages).toHaveBeenCalledTimes(3);
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(1999));
  expect(pendingMessages).toHaveBeenCalledTimes(3);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(pendingMessages).toHaveBeenCalledTimes(4);
  await act(() => vi.advanceTimersByTimeAsync(3999));
  expect(pendingMessages).toHaveBeenCalledTimes(4);
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(pendingMessages).toHaveBeenCalledTimes(5);
});

it('ignores late responses after viewer or Agent changes and never overlaps polling', async () => {
  const { pendingMessages, draw } = fixture();
  let finish!: (result: PendingChannelMessages) => void;
  pendingMessages.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(draw());
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(pendingMessages).toHaveBeenCalledOnce();
  const signal = pendingMessages.mock.calls[0]![2].signal as AbortSignal;
  pendingMessages.mockResolvedValue({ status: 'redacted' });
  view.rerender(draw({ viewerId: 'another-owner' }));
  expect(signal.aborted).toBe(true);
  await act(async () => finish(value()));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  view.rerender(draw({ agentId: 'agt_other' }));
  await flush();
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
});

it('refuses a snapshot for another Agent or generation and renders source text as plain text', async () => {
  const { pendingMessages, draw } = fixture();
  pendingMessages.mockResolvedValueOnce({ ...value(), agent_id: 'agt_other' });
  const view = render(draw());
  await flush();
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  pendingMessages.mockResolvedValueOnce({ ...value(), generation: 2 });
  await act(() => vi.advanceTimersByTimeAsync(2000));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  const snapshot = value();
  if (snapshot.status !== 'available') throw new Error('fixture');
  pendingMessages.mockResolvedValue({
    ...snapshot,
    messages: [
      { ...snapshot.messages[0], text: '<img src=x onerror=alert(1)>', text_truncated: true },
    ],
  });
  await act(() => vi.advanceTimersByTimeAsync(4000));
  expect(screen.getByText('<img src=x onerror=alert(1)>…')).toBeInTheDocument();
  expect(view.container.querySelector('img')).toBeNull();
});

it('pauses hidden-tab polling and clears previews before resuming current access checks', async () => {
  const { pendingMessages, draw } = fixture();
  render(draw());
  await flush();
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(pendingMessages).toHaveBeenCalledOnce();
  visibility.mockReturnValue('visible');
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  await flush();
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
  expect(pendingMessages).toHaveBeenCalledTimes(2);
});

it('resolves the real logical provenance hash to an allocation through the SDK contract', async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname;
    if (path === '/api/v1/bridges/brg_one/partitions')
      return Response.json({ items: [partitionRow()], next_cursor: null });
    if (path === `/api/v1/bridges/brg_one/partitions/${allocation}/pending-messages`)
      return Response.json(value());
    throw new Error('Unexpected endpoint');
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  render(
    <CmaClientProvider client={agentClient({ bridgeViews: sdk })}>
      <ChannelPendingMessages
        agentId="agt_one"
        viewerId="owner"
        provenance={provenance}
        cursor={20}
        enabled
      />
    </CmaClientProvider>,
  );
  await flush();
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(fetch.mock.calls.map(([input]) => String(input)).join(' ')).not.toContain(
    logicalPartition,
  );
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(fetch).toHaveBeenCalledTimes(3);
  expect(String(fetch.mock.calls[2]![0])).toContain(`/${allocation}/pending-messages`);
});

it('finds the unique current creating allocation across pages and excludes reused or historical targets', async () => {
  const { pendingMessages, listPartitions, draw } = fixture();
  listPartitions
    .mockResolvedValueOnce({
      items: [
        { ...partitionRow(), id: 'bral_old', current: false },
        { ...partitionRow(), id: 'bral_other_generation', generation: 2 },
        { ...partitionRow(), id: 'bral_reused', placement: 'existing_agent' },
      ],
      next_cursor: 'page2',
    })
    .mockResolvedValueOnce({ items: [partitionRow()], next_cursor: null });
  render(draw());
  await flush();
  expect(listPartitions).toHaveBeenCalledTimes(2);
  expect(listPartitions.mock.calls[1]![1].cursor).toBe('page2');
  expect(pendingMessages).toHaveBeenCalledWith('brg_one', allocation, {
    signal: expect.any(AbortSignal),
  });
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
});

it.each(['missing', 'ambiguous', 'redacted', 'repeated_cursor'])(
  'withholds previews when allocation discovery is %s',
  async (failure) => {
    const { pendingMessages, listPartitions, draw } = fixture();
    const other = { ...partitionRow(), id: 'bral_' + 'a'.repeat(64) };
    listPartitions.mockResolvedValue({
      items:
        failure === 'missing'
          ? []
          : failure === 'ambiguous'
            ? [partitionRow(), other]
            : failure === 'redacted'
              ? [partitionRow(), { ...other, target: { status: 'redacted' } }]
              : [],
      next_cursor: failure === 'repeated_cursor' ? 'repeated' : null,
    });
    render(draw());
    await flush();
    expect(pendingMessages).not.toHaveBeenCalled();
    expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
    expect(listPartitions.mock.calls.length).toBeLessThanOrEqual(2);
  },
);

it('discards an allocation lookup completed after the viewer changes', async () => {
  const { pendingMessages, listPartitions, draw } = fixture();
  let finish!: (value: unknown) => void;
  listPartitions.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const view = render(draw());
  await flush();
  const signal = listPartitions.mock.calls[0]![1].signal as AbortSignal;
  listPartitions.mockResolvedValue({ items: [], next_cursor: null });
  view.rerender(draw({ viewerId: 'another-owner' }));
  expect(signal.aborted).toBe(true);
  await act(async () => finish({ items: [partitionRow()], next_cursor: null }));
  expect(pendingMessages).not.toHaveBeenCalled();
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
});
