import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentDeletionNotice } from './AgentDeletionNotice';
import type { DeletionRequest } from './deletion';
import { AGENT_DETAIL, agentClient } from './test/agent';

const request: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: AGENT_DETAIL.id },
  key: 'delete-agent',
  accepted: true,
  acceptedAt: '2026-11-28T00:00:00Z',
};

afterEach(() => vi.useRealTimers());

it('keeps an accepted delete visible while the Agent is still stopped and ticks across remount', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-28T00:00:05Z'));
  const readAgent = vi.fn(async () => ({ ...AGENT_DETAIL, status: 'stopped' as const }));
  const client = agentClient({ readAgent, readAgentEvents: async () => ({ items: [] }) });
  const props = { client, request, onViewStatus: vi.fn(), disabled: false };
  const first = render(<AgentDeletionNotice {...props} />);
  await act(async () => {});
  expect(screen.getByRole('status')).toHaveTextContent('Deleting Agent');
  expect(screen.getByText('Elapsed 5s')).toBeVisible();
  expect(screen.getByRole('status').querySelector('.statedot__ping')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(screen.getByText('Elapsed 6s')).toBeVisible();
  expect(readAgent.mock.calls.length).toBeGreaterThan(1);
  first.unmount();
  await act(() => vi.advanceTimersByTimeAsync(4000));
  render(<AgentDeletionNotice {...props} />);
  await act(async () => {});
  expect(screen.getByText('Elapsed 10s')).toBeVisible();
});

it('shows a deletion failure reason only after the Agent confirms the cleanup failure', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-28T00:00:05Z'));
  const failure = { class: 'cleanup_failed', detail: 'The worktree could not be removed.' };
  const event = {
    seq: 1,
    appendedAt: '2026-11-28T00:00:04Z',
    kind: { type: 'work_step_failed', step: 'remove_worktree', failure },
  };
  const readAgent = vi.fn(async () => ({ ...AGENT_DETAIL, status: 'stopped' as const }));
  const client = agentClient({
    readAgent,
    readAgentEvents: async (_id, options) => ({ items: options?.cursor ? [] : [event] }),
  });
  render(
    <AgentDeletionNotice
      client={client}
      request={request}
      onViewStatus={vi.fn()}
      disabled={false}
    />,
  );
  await act(async () => {});
  expect(screen.getByRole('status')).toHaveTextContent('Deleting Agent');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  // An existing Agent failure unrelated to cleanup cannot settle the delete.
  client.readAgent = async () => ({
    ...AGENT_DETAIL,
    status: 'failed',
    failure: { class: 'startup_failed', detail: 'Startup failed.' },
  });
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.getByRole('status')).toHaveTextContent('Deleting Agent');
  client.readAgent = async () => ({ ...AGENT_DETAIL, status: 'failed', failure });
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Agent deletion failed. The worktree could not be removed.',
  );
  expect(screen.queryByText(/^Elapsed/)).not.toBeInTheDocument();
});
