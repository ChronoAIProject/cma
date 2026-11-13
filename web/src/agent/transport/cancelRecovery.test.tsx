import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useAgentController } from '../controller';
import { binding, harness, item, json, page } from './controller.fixture';

beforeEach(() => sessionStorage.clear());

it.each(['accepted input', 'no input', 'denied identity'] as const)(
  'recovers a retained Cancel without unrelated background checks with %s',
  async (scenario) => {
    const h = harness();
    h.snapshot.mockImplementation(async () =>
      json(page([item(1, { type: 'turn_started', turn_sequence: 1 }, binding)], 1)),
    );
    h.control.mockRejectedValueOnce(new TypeError('lost receipt'));
    const view = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
    await act(() => view.result.current.interrupt());
    expect(view.result.current.pendingControl).toBe(true);
    const state = view.result.current.state;
    if (state.status !== 'ready' || !state.v2) throw new Error('Expected a v2 session.');
    if (scenario === 'accepted input') {
      const input = state.v2.journal.retain('Previously accepted message');
      state.v2.journal.accepted(input.key, { responseId: 'resp_1', commandId: 'cmd_1' });
    }
    let release!: (user: string) => void;
    let reached!: () => void;
    const ready = new Promise<void>((resolve) => {
      reached = resolve;
    });
    h.currentUser.mockImplementationOnce(() => {
      reached();
      return new Promise((resolve) => {
        release = resolve;
      });
    });
    const background = state.v2.authorize();
    await ready;
    if (scenario === 'denied identity')
      h.currentUser.mockRejectedValueOnce(new Error('identity unavailable'));
    try {
      await act(() => view.result.current.recoverPending!());
      expect(h.control).toHaveBeenCalledTimes(scenario === 'denied identity' ? 1 : 2);
      expect(h.observe).toHaveBeenCalledOnce();
      expect(h.post).not.toHaveBeenCalled();
      expect(h.interrupt).not.toHaveBeenCalled();
      expect(view.result.current.pendingControl).toBe(scenario === 'denied identity');
      expect(view.result.current.state).toMatchObject({ conversation: { activeRunId: '1' } });
      if (scenario === 'denied identity')
        expect(view.result.current.actionError).toContain('identity unavailable');
      else {
        const first = h.control.mock.calls[0]![1]!;
        const second = h.control.mock.calls[1]![1]!;
        expect(second.body).toBe(first.body);
        expect(new Headers(second.headers).get('idempotency-key')).toBe(
          new Headers(first.headers).get('idempotency-key'),
        );
      }
    } finally {
      release('alice');
      await act(() => background);
      view.unmount();
    }
  },
);
