import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentSummary } from '../api/types';
import { AGENT_DETAIL, AGENT_USAGE, agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

const summary: AgentSummary = {
  ...workspaceFixture().agents[0]!,
  title: 'Generated introduction',
  updatedAt: '2026-09-06T00:06:00Z',
};

it.each([
  { id: 'agt_other' },
  { sandboxId: 'sbx_other' },
  { workspaceId: 'wsp_other' },
  { worktreeId: 'wkt_other' },
  { title: undefined },
  { title: '   ' },
  { updatedAt: '2026-09-06T00:04:00Z' },
  { updatedAt: 'invalid' },
])('ignores unrelated, untitled or stale navigation metadata %j', async (change) => {
  vi.useFakeTimers();
  const client = agentClient();
  const snapshot = { ...summary, ...change };
  const { result, unmount } = renderHook(() =>
    useAgentController(client, 'agt_1', { summary: snapshot }),
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(result.current.state.status).toBe('ready');
  if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
  expect(result.current.state.detail.title).toBe(AGENT_DETAIL.title);
  unmount();
});

it('changes only title metadata and retains it across missing or older navigation snapshots', async () => {
  vi.useFakeTimers();
  const client = agentClient();
  const initialProps: { snapshot: AgentSummary | undefined } = { snapshot: undefined };
  const { result, rerender, unmount } = renderHook(
    ({ snapshot }: { snapshot: AgentSummary | undefined }) =>
      useAgentController(client, 'agt_1', { summary: snapshot }),
    { initialProps },
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
  const before = result.current.state;
  rerender({ snapshot: summary });
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(result.current.state).toEqual({
    ...before,
    detail: { ...before.detail, title: summary.title, updatedAt: summary.updatedAt },
  });
  const updated = result.current.state;
  for (const snapshot of [
    undefined,
    { ...summary, title: 'Old title', updatedAt: AGENT_DETAIL.updatedAt },
    { ...summary, title: undefined },
  ]) {
    rerender({ snapshot });
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(result.current.state).toBe(updated);
  }
  unmount();
});

it('does not carry a previous Agent title through navigation', async () => {
  vi.useFakeTimers();
  const client = agentClient({
    readAgent: async (id) => ({ ...AGENT_DETAIL, id, title: undefined }),
    readAgentTranscript: async (id) => ({ agentId: id, throughSeq: 0, turns: [] }),
    readAgentUsage: async (id) => ({ ...AGENT_USAGE, agentId: id }),
  });
  const { result, rerender, unmount } = renderHook(
    ({ id }) => useAgentController(client, id, { summary }),
    { initialProps: { id: 'agt_1' } },
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  if (result.current.state.status !== 'ready') throw new Error('Agent did not load');
  expect(result.current.state.detail.title).toBe(summary.title);
  rerender({ id: 'agt_2' });
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(result.current.state).toMatchObject({
    status: 'ready',
    detail: { id: 'agt_2', title: undefined },
  });
  unmount();
});
