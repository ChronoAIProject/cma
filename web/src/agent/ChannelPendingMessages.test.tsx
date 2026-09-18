import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AgentProvenance, PendingChannelMessages } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import type { BridgeViewsClient } from '../bridges/client';
import { ChannelPendingMessages } from './ChannelPendingMessages';
import type { ProvenanceState } from './provenance';

const value = (): PendingChannelMessages => ({
  status: 'available',
  bridge_id: 'brg_one',
  partition_id: 'bral_one',
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
      partition_id: 'bral_one',
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
  const client = agentClient({
    bridgeViews: { bridges: { pendingMessages } } as unknown as BridgeViewsClient,
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
  return { pendingMessages, draw };
}

it('shows accepted text before a turn, polls without reload, and invalidates it when replay advances', async () => {
  const { pendingMessages, draw } = fixture();
  const view = render(draw());
  await flush();
  expect(screen.getByText('Queued from Telegram')).toBeInTheDocument();
  expect(screen.getByText('Telegram message received · Waiting to start')).toBeInTheDocument();
  expect(pendingMessages).toHaveBeenCalledWith('brg_one', 'bral_one', {
    signal: expect.any(AbortSignal),
  });
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(pendingMessages).toHaveBeenCalledTimes(2);
  let finish!: (result: PendingChannelMessages) => void;
  pendingMessages.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  view.rerender(draw({ cursor: 22 }));
  expect(screen.queryByText('Queued from Telegram')).not.toBeInTheDocument();
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
