import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { BridgePartition, BridgePartitionDetail } from '@chronoai/cma-sdk';
import { BridgePartitions } from './BridgePartitions';
import { bridgeTransport } from './fixtures.test-support';
const allocation = `bral_${'1'.repeat(64)}`;
const row: BridgePartition = {
  id: allocation,
  bridge_id: 'brg_one',
  generation: 1,
  revision: 1,
  config_revision: 1,
  current: true,
  state: 'ready',
  placement: 'existing_agent',
  context_sharing: 'fixed_agent',
  target: {
    status: 'available',
    ids: {
      sandbox_id: 'sbx_exact',
      workspace_id: 'wks_exact',
      worktree_id: 'wkt_exact',
      agent_id: 'agt_exact',
    },
    present: ['sandbox', 'workspace', 'worktree', 'agent'],
  },
  source: {
    status: 'available',
    platform: 'telegram',
    chat_id: '-100123',
    sender_id: 'sender-one',
    sender_display_name: '<img src=x onerror=alert(1)>',
    conversation_type: 'group',
  },
  observed_at_ms: 1000,
};
const detail: BridgePartitionDetail = {
  partition: row,
  activity: {
    status: 'available',
    agent_status: 'waiting_approval',
    pending: [
      {
        command_id: 'cmd_exact',
        response_id: 'resp_exact',
        config_revision: 1,
        agent_order: 1,
        status: 'waiting_action',
        waiting_for: 'approval',
        turn_sequence: 1,
        delivery: 'execution_observed',
        admitted_at: '2026-09-15T06:00:00Z',
      },
    ],
    latest: null,
    source_unavailable: true,
    pending_truncated: false,
    observed_at_ms: 1000,
  },
};
function fixture() {
  const state = {
    detail: structuredClone(detail),
    denied: false,
    pause: undefined as Promise<Response> | undefined,
  };
  const transport = bridgeTransport((url) => {
    if (url.pathname.endsWith(`/partitions/${allocation}`))
      return state.pause ?? Response.json(state.detail);
    if (url.pathname.endsWith('/partitions'))
      return state.denied
        ? Response.json(
            {
              type: 'about:blank',
              title: 'Forbidden',
              status: 403,
              code: 'bridge_management_required',
              detail: 'private diagnostic',
            },
            { status: 403 },
          )
        : Response.json({ items: [row], next_cursor: null });
    return undefined;
  });
  return {
    ...transport,
    state,
    props: {
      client: transport.client,
      viewerId: 'owner',
      id: 'brg_one',
      ownerScope: 'person:owner',
      revision: 1,
      allowed: true,
      onNavigate: vi.fn(),
    },
  };
}

it('shows literal source labels, exact conversation links and approval correlation without a write', async () => {
  const f = fixture();
  const user = userEvent.setup();
  render(<BridgePartitions {...f.props} />);
  expect(f.fetch).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Show partitions' }));
  const conversation = await screen.findByRole('link', {
    name: 'Open Agent conversation agt_exact',
  });
  expect(conversation).toHaveAttribute('href', '/agents/agt_exact');
  expect(screen.getByText(/<img src=x onerror=alert\(1\)>/)).toBeInTheDocument();
  expect(document.querySelector('img')).toBeNull();
  await user.click(conversation);
  expect(f.props.onNavigate).toHaveBeenCalledWith({ kind: 'agent', agentId: 'agt_exact' });
  await user.click(screen.getByRole('button', { name: 'Inspect generation 1' }));
  await screen.findByText('Waiting for approval');
  expect(screen.getByText(/Response resp_exact/)).toBeInTheDocument();
  expect(screen.getByText(/This activity view is incomplete/)).toBeInTheDocument();
  expect(
    screen.getByText(/Check the Agent conversation for current connection status/),
  ).toBeInTheDocument();
  expect(f.fetch.mock.calls.every(([, init]) => (init?.method ?? 'GET') === 'GET')).toBe(true);
});

it('clears selected source and activity on viewer change and discards a late detail response', async () => {
  const f = fixture();
  let resolve!: (response: Response) => void;
  f.state.pause = new Promise((done) => {
    resolve = done;
  });
  const user = userEvent.setup();
  const view = render(<BridgePartitions {...f.props} />);
  await user.click(screen.getByRole('button', { name: 'Show partitions' }));
  await user.click(await screen.findByRole('button', { name: 'Inspect generation 1' }));
  await screen.findByText('Checking selected partition and conversation access');
  view.rerender(<BridgePartitions {...f.props} viewerId="another" allowed={false} />);
  await act(async () => resolve(Response.json(detail)));
  expect(screen.queryByText(/sender-one/)).not.toBeInTheDocument();
  expect(screen.queryByText(/resp_exact/)).not.toBeInTheDocument();
  view.rerender(<BridgePartitions {...f.props} viewerId="another" />);
  expect(screen.getByRole('button', { name: 'Show partitions' })).toBeInTheDocument();
});

it('removes stale rows when refreshed authority fails and displays typed source redaction', async () => {
  const f = fixture();
  f.state.detail = {
    partition: { ...row, target: { status: 'redacted' }, source: { status: 'redacted' } },
    activity: { status: 'redacted' },
  };
  const user = userEvent.setup();
  render(<BridgePartitions {...f.props} />);
  await user.click(screen.getByRole('button', { name: 'Show partitions' }));
  await user.click(await screen.findByRole('button', { name: 'Inspect generation 1' }));
  await screen.findByText('Conversation activity is restricted by current resource access.');
  f.state.denied = true;
  await user.click(screen.getByRole('button', { name: 'Refresh partitions' }));
  await waitFor(() =>
    expect(screen.getByRole('alert')).toHaveTextContent('Partition status is unavailable.'),
  );
  expect(screen.queryByRole('list', { name: 'Bridge partitions' })).not.toBeInTheDocument();
  expect(screen.queryByText('private diagnostic')).not.toBeInTheDocument();
});
