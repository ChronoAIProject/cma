import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../api/types';
import type { ResourceViewsClient } from './client';
import { useAgentAccess } from './agentAccess';
import { retainLifecycleHandoff } from './lifecycleHandoff';

const detail = {
  id: 'agt_1',
  workspace: { id: 'wsp_1' },
  sandbox: { id: 'sbx_1' },
} as AgentDetail;

afterEach(() => {
  vi.useRealTimers();
});

it('opens from the verified lifecycle handoff while access revalidation is pending', async () => {
  const readAccessible = vi.fn(
    () =>
      new Promise<AgentDetail>((resolve) => {
        setTimeout(() => resolve(detail), 50);
      }),
  );
  const resourceViews = {
    agents: { readAccessible },
  } as unknown as ResourceViewsClient;
  retainLifecycleHandoff(resourceViews, 'alice', detail);
  const context = {};
  const view = renderHook(() => useAgentAccess(resourceViews, 'agt_1', 'alice', context));

  expect(view.result.current.personalReady).toBe(true);
  expect(view.result.current.personalReadOnly).toBe(false);
  expect(view.result.current.personalLineage).toEqual({
    agentId: 'agt_1',
    workspaceId: 'wsp_1',
    sandboxId: 'sbx_1',
  });

  await waitFor(() => expect(readAccessible).toHaveBeenCalledOnce());
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  expect(view.result.current.personalReady).toBe(true);
  expect(view.result.current.personalReadOnly).toBe(false);
});

it('does not reuse a lifecycle handoff for a different owner or client', () => {
  const first = {} as ResourceViewsClient;
  const second = {} as ResourceViewsClient;
  retainLifecycleHandoff(first, 'alice', detail);
  const alice = renderHook(() => useAgentAccess(first, 'agt_1', 'alice'));
  const bob = renderHook(() => useAgentAccess(first, 'agt_1', 'bob'));
  const otherClient = renderHook(() => useAgentAccess(second, 'agt_1', 'alice'));
  expect(alice.result.current.personalReady).toBe(true);
  expect(bob.result.current.personalReady).toBe(false);
  expect(otherClient.result.current.personalReady).toBe(false);
});
