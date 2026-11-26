import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { CommandKeys } from '../api/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { useAgentActions } from './actions';
import type { AgentActionOptions } from './actionOptions';
import { conversationFromTranscript } from './conversation';

function options(): AgentActionOptions {
  return {
    agentId: 'agt_1',
    client: agentClient({ runAgent: vi.fn().mockResolvedValue(undefined) }),
    historyIncomplete: false,
    commandsBlocked: false,
    activeTurn: false,
    inputAvailability: { allowed: true },
    actionAbortRef: { current: undefined },
    actionKindRef: { current: undefined },
    replayAbortRef: { current: undefined },
    stateRef: {
      current: {
        status: 'ready',
        detail: AGENT_DETAIL,
        workspace: workspaceFixture(),
        conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
      },
    },
    keys: new CommandKeys(),
    setPending: vi.fn(),
    setActionError: vi.fn(),
    setReplayRevision: vi.fn(),
    setRequestRevision: vi.fn(),
    applyLive: vi.fn(),
    reload: vi.fn(),
    refreshDetail: vi.fn(),
    errorMessage: String,
    onIdentityChange: () => false,
    options: { viewerId: 'alice' },
  };
}

it.each(['missing Agent', 'loading', 'wrong Agent', 'input held', 'busy', 'starting'] as const)(
  'reports an ordinary message not dispatched when %s prevents admission',
  async (reason) => {
    const input = options();
    const other = new AbortController();
    const value = {
      ...input,
      agentId: reason === 'missing Agent' ? undefined : input.agentId,
      inputAvailability: { allowed: reason !== 'input held' },
    };
    if (reason === 'loading') input.stateRef.current = { status: 'loading' };
    if (
      (reason === 'wrong Agent' || reason === 'starting') &&
      input.stateRef.current.status === 'ready'
    )
      input.stateRef.current = {
        ...input.stateRef.current,
        detail: {
          ...AGENT_DETAIL,
          ...(reason === 'wrong Agent' ? { id: 'agt_other' } : { status: 'starting' as const }),
        },
      };
    if (reason === 'busy') input.actionAbortRef.current = other;
    const view = renderHook(() => useAgentActions(value));
    const refused = vi.fn();
    const accepted = vi.fn();
    await act(() => view.result.current.run('Keep my draft', accepted, undefined, refused));
    expect(refused).toHaveBeenCalledExactlyOnceWith();
    expect(accepted).not.toHaveBeenCalled();
    expect(input.client.runAgent).not.toHaveBeenCalled();
    expect(input.setPending).not.toHaveBeenCalled();
    expect(input.setActionError).not.toHaveBeenCalled();
    expect(other.signal.aborted).toBe(false);
    if (reason === 'busy') expect(input.actionAbortRef.current).toBe(other);
  },
);

it.each(['send', 'steer', 'interrupt', 'approval'] as const)(
  'keeps %s callback ownership when an old action context returns',
  async (mode) => {
    const input = options();
    const view = renderHook((value) => useAgentActions(value), { initialProps: input });
    const old = view.result.current.run;
    view.rerender({ ...input, inputAvailability: { allowed: false } });
    view.rerender(input);
    const refused = vi.fn();
    await act(() =>
      old(
        mode === 'approval' ? { id: 'approval-local', decision: 'allow' } : 'Old input',
        undefined,
        undefined,
        refused,
        mode === 'approval' ? 'send' : mode,
      ),
    );
    expect(refused).toHaveBeenCalledTimes(mode === 'send' ? 1 : 0);
    expect(input.client.runAgent).not.toHaveBeenCalled();
    expect(input.setPending).not.toHaveBeenCalled();
  },
);

it('retains the server guidance for a temporarily unavailable Agent', async () => {
  const input = options();
  vi.mocked(input.client.runAgent).mockRejectedValue(
    new ApiError(409, 'Wait for this Agent to become ready.', 'agent_not_ready'),
  );
  const view = renderHook(() => useAgentActions(input));
  await act(() => view.result.current.run('Keep my draft'));
  expect(input.setActionError).toHaveBeenLastCalledWith(
    'ApiError: Wait for this Agent to become ready.',
    undefined,
  );
});
