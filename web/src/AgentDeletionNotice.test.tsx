import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentDeletionNotice } from './AgentDeletionNotice';
import type { AgentDetail } from './api/types';
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

it('keeps checking an already failed Agent after a matching cleanup error until deletion completes', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-28T00:00:05Z'));
  const failure = { class: 'cleanup_failed', detail: 'The worktree could not be removed.' };
  const event = {
    seq: 1,
    appendedAt: '2026-11-28T00:00:04Z',
    kind: { type: 'work_step_failed', step: 'ensure_process_closed', failure },
  };
  let detail: AgentDetail = { ...AGENT_DETAIL, status: 'failed', failure };
  const readAgent = vi.fn(async () => detail);
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
  expect(
    screen.getByText('Cleanup reported a problem. The worktree could not be removed.'),
  ).toBeVisible();
  await act(() => vi.advanceTimersByTimeAsync(1_000));
  expect(screen.getByRole('status')).toHaveTextContent('Deleting Agent');
  expect(screen.getByText('Elapsed 6s')).toBeVisible();
  detail = { ...detail, status: 'deleted', worktree: { ...detail.worktree, status: 'removed' } };
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.queryByRole('region', { name: 'Agent deletion' })).not.toBeInTheDocument();
  const reads = readAgent.mock.calls.length;
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect(readAgent).toHaveBeenCalledTimes(reads);
});
