import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useConsoleAgentAccess } from './consoleAgentAccess';
import { useAgentController } from '../agent/controller';
import { harness } from '../agent/transport/controller.fixture';
import { WIRE_AGENT } from '../test/rest';
import { AGENT_DETAIL } from '../test/agent';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it('reopens retained personal routes without preparing another transport and resets on client replacement', async () => {
  const h = harness();
  const sdk = createClient({ fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }) });
  const first = { ...h.client, resourceViews: sdk };
  const prepare = vi.spyOn(h.client.agentTransport!, 'prepareOpening');
  const view = renderHook(({ client, id }) => useConsoleAgentAccess(client, id, 'alice'), {
    initialProps: { client: first, id: 'agt_1' as string | undefined },
  });
  await waitFor(() => expect(view.result.current.agentAccess.personalReady).toBe(true));
  expect(prepare).toHaveBeenCalledOnce();
  view.rerender({ client: first, id: undefined });
  view.rerender({ client: first, id: 'agt_1' });
  await waitFor(() => expect(view.result.current.agentAccess.state.status).toBe('ready'));
  expect(prepare).toHaveBeenCalledOnce();
  view.rerender({ client: { ...first }, id: 'agt_1' });
  await waitFor(() => expect(view.result.current.agentAccess.personalReady).toBe(true));
  expect(prepare).toHaveBeenCalledTimes(2);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});

it.each(['personal', 'shared', 'denied'] as const)(
  'keeps protected loading behind %s access classification while selecting transport',
  async (access) => {
    const h = harness();
    const sdk = createClient({ fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }) });
    const detail = await sdk.agents.readAccessible('agt_1');
    let finish!: (value: typeof detail) => void;
    let refuse!: (error: Error) => void;
    vi.spyOn(sdk.agents, 'readAccessible').mockReturnValue(
      new Promise((resolve, reject) => {
        finish = resolve;
        refuse = reject;
      }),
    );
    vi.spyOn(sdk.resourceAccess, 'effective').mockResolvedValue({
      capabilities: ['view_details', 'view_history'],
      owner: { kind: 'person', is_current_person: false },
      revalidate_at: null,
      policy_revision: 1,
    });
    const client = { ...h.client, resourceViews: sdk };
    const agentRead = vi.spyOn(client, 'readAgent');
    const view = renderHook(() => {
      const state = useConsoleAgentAccess(client, 'agt_1', 'alice');
      const controller = useAgentController(client, state.personalAgentId, {
        viewerId: 'alice',
        suspended: state.accessUncertain,
      });
      return { state, controller };
    });
    await waitFor(() => expect(h.eligibility).toHaveBeenCalledOnce());
    expect(h.snapshot).not.toHaveBeenCalled();
    expect(agentRead).not.toHaveBeenCalled();
    await act(async () => {
      if (access === 'denied') refuse(new ApiError(403, 'Access denied'));
      else finish(access === 'shared' ? { ...detail, access: 'shared' } : detail);
    });
    if (access === 'personal') {
      await waitFor(() => expect(view.result.current.controller.state.status).toBe('ready'));
      expect(agentRead).toHaveBeenCalledWith(AGENT_DETAIL.id, expect.anything());
      expect(h.snapshot).toHaveBeenCalled();
    } else {
      expect(view.result.current.state.personalAgentId).toBeUndefined();
      expect(agentRead).not.toHaveBeenCalled();
      expect(h.snapshot).not.toHaveBeenCalled();
      expect(view.result.current.controller.state.status).toBe('idle');
    }
    expect(h.post).not.toHaveBeenCalled();
    view.unmount();
  },
);
