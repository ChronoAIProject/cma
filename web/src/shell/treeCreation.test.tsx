import { act, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import type { AgentCreationCategory, AgentProvenance } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import { navigationFixture, workspaceFixture } from '../test/hierarchy';
import { Tree } from './Tree';
import { CreationSummary, useTreeCreation } from './treeCreation';
import { TreeCreationScope } from './treeCreationScope';

function fixture(count = 6) {
  const workspace = workspaceFixture();
  const agent = workspace.agents[0]!;
  return navigationFixture().map((item) => ({
    ...item,
    workspaces: [
      {
        ...workspace,
        agents: Array.from({ length: count }, (_, index) => ({
          ...agent,
          id: `agt_${String(index)}`,
          title: `Agent ${String(index)}`,
        })),
      },
    ],
  }));
}

function facts(id: string, category: AgentCreationCategory = 'manual'): AgentProvenance {
  return {
    agent_id: id,
    created_at: '2026-09-06T00:00:00Z',
    creation_origin: category,
    details: {
      created_by: { kind: 'human', subject: 'private-subject' },
      trigger: null,
      channel_bridge: null,
      workspace_profile_index: null,
    },
  };
}

function deferred() {
  let resolve!: (value: AgentProvenance) => void;
  const promise = new Promise<AgentProvenance>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('limits optional reads to four and retains categories without private provenance details', async () => {
  const pending = Array.from({ length: 6 }, deferred);
  const readAgentProvenance = vi.fn((id: string) => pending[Number(id.slice(4))]!.promise);
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result } = renderHook(() => useTreeCreation('viewer', fixture(), true), { wrapper });
  expect(readAgentProvenance).toHaveBeenCalledTimes(4);
  await act(async () => pending[1]!.resolve(facts('agt_1', 'trigger')));
  expect(readAgentProvenance).toHaveBeenCalledTimes(5);
  expect([...result.current]).toEqual([['agt_1', 'trigger']]);
  await act(async () => pending[0]!.resolve(facts('agt_0')));
  expect(readAgentProvenance).toHaveBeenCalledTimes(6);
  await act(async () =>
    pending.forEach((item, index) => item.resolve(facts(`agt_${String(index)}`))),
  );
  expect(result.current.size).toBe(6);
  expect(JSON.stringify([...result.current])).not.toContain('private-subject');
});

it('clears every category on viewer/client changes and refuses stale or mismatched replies', async () => {
  const pending = deferred();
  const readAgentProvenance = vi
    .fn()
    .mockResolvedValueOnce(facts('agt_0'))
    .mockReturnValue(pending.promise);
  let client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, rerender } = renderHook(
    ({ viewer, enabled }) => useTreeCreation(viewer, fixture(1), enabled),
    {
      initialProps: { viewer: 'one', enabled: true },
      wrapper,
    },
  );
  await waitFor(() => expect(result.current.get('agt_0')).toBe('manual'));
  rerender({ viewer: 'two', enabled: true });
  expect(result.current.size).toBe(0);
  await act(async () => pending.resolve(facts('another-agent')));
  expect(result.current.get('agt_0')).toBe('unavailable');
  rerender({ viewer: 'two', enabled: false });
  expect(result.current.size).toBe(0);
  client = { ...client };
  rerender({ viewer: 'two', enabled: true });
  expect(result.current.size).toBe(0);
  await act(async () => {});
  expect(result.current.get('agt_0')).toBe('unavailable');
});

it('retries unavailable categories, retains known origins, skips hidden pages and cancels on unmount', async () => {
  vi.useFakeTimers();
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  const readAgentProvenance = vi
    .fn()
    .mockRejectedValueOnce(new Error('Denied'))
    .mockResolvedValue(facts('agt_0'));
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, unmount } = renderHook(() => useTreeCreation('viewer', fixture(1), true), {
    wrapper,
  });
  expect(readAgentProvenance).not.toHaveBeenCalled();
  hidden.mockReturnValue(false);
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  expect(result.current.get('agt_0')).toBe('unavailable');
  await act(async () => vi.advanceTimersByTimeAsync(60_000));
  expect(result.current.get('agt_0')).toBe('manual');
  await act(async () => vi.advanceTimersByTimeAsync(120_000));
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  expect(readAgentProvenance).toHaveBeenCalledTimes(2);
  unmount();
  expect(readAgentProvenance.mock.calls[0]?.[1].signal.aborted).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(60_000));
  expect(readAgentProvenance).toHaveBeenCalledTimes(2);
});

it('does not replay pending reads or re-fetch when only Agent activity changes', async () => {
  const pending = deferred();
  const readAgentProvenance = vi.fn().mockReturnValue(pending.promise);
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { rerender, unmount } = renderHook(({ items }) => useTreeCreation('viewer', items, true), {
    initialProps: { items: fixture(1) },
    wrapper,
  });
  rerender({ items: fixture(1) });
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  expect(readAgentProvenance).toHaveBeenCalledOnce();
  unmount();
  await act(async () => pending.resolve(facts('agt_0')));
  expect(readAgentProvenance).toHaveBeenCalledOnce();
});

it('hides an incomplete child summary', () => {
  const categories = new Map<string, AgentCreationCategory | 'unavailable'>([
    ['agt_0', 'manual'],
    ['agt_1', 'trigger'],
    ['agt_2', 'legacy_unknown'],
    ['agt_3', 'unavailable'],
  ]);
  const { rerender } = render(<CreationSummary ids={['agt_0', 'agt_3']} facts={categories} />);
  expect(screen.queryByTitle(/Agent creation/)).not.toBeInTheDocument();
  rerender(<CreationSummary ids={['agt_0', 'agt_1']} facts={categories} />);
  expect(screen.getByTitle('Agent creation origins: 1 manual, 1 trigger')).toBeVisible();
});

it('shares immutable origins between retained trees and reads only newly appearing Agents', async () => {
  vi.useFakeTimers();
  const readAgentProvenance = vi.fn(async (id: string) => facts(id));
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>
      <TreeCreationScope viewerId="viewer">{children}</TreeCreationScope>
    </CmaClientProvider>
  );
  const { result, rerender } = renderHook(
    ({ view, count }) => ({
      tasks: useTreeCreation('viewer', fixture(count), view === 'tasks'),
      environments: useTreeCreation('viewer', fixture(count), view === 'environments'),
    }),
    { wrapper, initialProps: { view: 'tasks', count: 2 } },
  );
  await act(async () => {});
  expect(readAgentProvenance).toHaveBeenCalledTimes(2);
  expect(result.current.environments.size).toBe(0);
  rerender({ view: 'environments', count: 2 });
  await act(async () => {});
  expect(result.current.environments.size).toBe(2);
  expect(result.current.tasks.size).toBe(0);
  await act(() => vi.advanceTimersByTimeAsync(120000));
  expect(readAgentProvenance).toHaveBeenCalledTimes(2);
  rerender({ view: 'tasks', count: 3 });
  await act(async () => {});
  expect(readAgentProvenance).toHaveBeenCalledTimes(3);
  expect(result.current.tasks.size).toBe(3);
});

it('shows distinct sibling badges and preserves whole-parent summaries without tree filters', async () => {
  const readAgentProvenance = vi.fn((id: string) =>
    Promise.resolve(facts(id, id === 'agt_0' ? 'manual' : 'trigger')),
  );
  const client = { ...unavailableClient, readAgentProvenance };
  const permissions = {
    createWorkspace: false,
    createAgent: false,
    pauseSandbox: false,
    deleteSandbox: false,
    removeWorkspace: false,
    stopAgent: false,
    deleteAgent: false,
    exportHistory: false,
  };
  render(
    <CmaClientProvider client={client}>
      <Tree
        viewerId="origins-test"
        items={fixture(2)}
        route={{ kind: 'home' }}
        permissions={permissions}
        onNavigate={vi.fn()}
        onAction={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByTitle('Created manually')).toBeVisible();
  expect(screen.getByTitle('Created by Trigger')).toBeVisible();
  const manualRow = screen.getByRole('treeitem', { name: /Agent 0/ });
  expect(within(manualRow).queryByTitle('Created by Trigger')).not.toBeInTheDocument();
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.getByText('Agent 0')).toBeVisible();
  expect(screen.getByText('Agent 1')).toBeVisible();
  expect(screen.getAllByTitle('Agent creation origins: 1 manual, 1 trigger')).toHaveLength(2);
  expect(screen.queryByText('private-subject')).not.toBeInTheDocument();
});
