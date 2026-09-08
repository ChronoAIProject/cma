import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import type { AgentSource } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import { AgentSourceLabel, useAgentSource } from './source';

const value: AgentSource = {
  agent_id: 'agt_one',
  origin: { type: 'trigger', trigger_id: 'trg_one', launch_id: 'lch_one' },
  trigger: { title: 'Project introduction', public_id: 'pub_one', revision: 2, quick_messages: [] },
};

it('shows original source names and native links, never internal IDs as labels', () => {
  const { rerender } = render(
    <AgentSourceLabel source={{ status: 'ready', agentId: 'agt_one', value }} />,
  );
  expect(screen.getByRole('link', { name: 'Project introduction' })).toHaveAttribute(
    'href',
    '/t/pub_one',
  );
  expect(screen.queryByText('trg_one')).not.toBeInTheDocument();
  rerender(
    <AgentSourceLabel
      source={{
        status: 'ready',
        agentId: 'agt_one',
        value: { agent_id: 'agt_one', origin: { type: 'manual' } },
      }}
    />,
  );
  expect(screen.getByText('Created manually')).toBeVisible();
  rerender(
    <AgentSourceLabel
      source={{
        status: 'ready',
        agentId: 'agt_one',
        value: { agent_id: 'agt_one', origin: { type: 'workspace_profile' } },
      }}
    />,
  );
  expect(screen.getByText('Created from Workspace Profile')).toBeVisible();
});

it('does not leak an earlier Agent source while the selected Agent is changing', async () => {
  let release!: (value: AgentSource) => void;
  const readAgentSource = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    )
    .mockResolvedValue({ agent_id: 'agt_two', origin: { type: 'manual' } });
  const client = { ...unavailableClient, readAgentSource };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, rerender } = renderHook(({ id }) => useAgentSource(id, true), {
    initialProps: { id: 'agt_one' },
    wrapper,
  });
  rerender({ id: 'agt_two' });
  await waitFor(() => expect(result.current.status).toBe('ready'));
  await act(async () => release(value));
  expect(result.current.agentId).toBe('agt_two');
  expect(result.current.status === 'ready' && result.current.value.origin.type).toBe('manual');
});

it('keeps source failures independent and removes previously loaded actions on access loss', async () => {
  const readAgentSource = vi
    .fn()
    .mockResolvedValueOnce(value)
    .mockRejectedValue(new Error('Denied'));
  const client = { ...unavailableClient, readAgentSource };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result } = renderHook(() => useAgentSource('agt_one', true), { wrapper });
  await waitFor(() => expect(result.current.status).toBe('ready'));
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  await waitFor(() => expect(result.current.status).toBe('unavailable'));
  expect(result.current).not.toHaveProperty('value');
});
