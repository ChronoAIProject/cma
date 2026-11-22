import { act, renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useResourceActions } from './ResourceActions';
import { agentClient } from './test/agent';
import { downloadAgentHistory } from './agent/historyExport';

vi.mock('./agent/historyExport', () => ({ downloadAgentHistory: vi.fn() }));

it.each([false, true])(
  'never reports a resource change for history export (failure: %s)',
  async (fails) => {
    const changed = vi.fn();
    const exportAgentHistory = vi.fn();
    if (fails) exportAgentHistory.mockRejectedValue(new Error('Export unavailable'));
    else exportAgentHistory.mockResolvedValue(new Blob(['downloaded history']));
    const client = agentClient({ exportAgentHistory });
    const view = renderHook(() => useResourceActions(client, changed));
    await act(async () => view.result.current.select({ kind: 'export_history', agentId: 'agt_1' }));
    expect(changed).not.toHaveBeenCalled();
    expect(exportAgentHistory).toHaveBeenCalledExactlyOnceWith('agt_1', {
      signal: expect.any(AbortSignal),
    });
    if (fails) expect(view.result.current.error).toBe('Export unavailable');
    else expect(downloadAgentHistory).toHaveBeenCalledWith('agt_1', expect.any(Blob));
  },
);

it('delivers a delayed acceptance to the current view callback after navigation', async () => {
  let accept!: () => void;
  const pauseSandbox = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        accept = resolve;
      }),
  );
  const client = agentClient({ pauseSandbox });
  const previous = vi.fn();
  const current = vi.fn();
  const view = renderHook(({ changed }) => useResourceActions(client, changed), {
    initialProps: { changed: previous },
  });
  act(() => view.result.current.select({ kind: 'pause_sandbox', sandboxId: 'sbx_1' }));
  view.rerender({ changed: current });
  await act(async () => accept());
  expect(previous).not.toHaveBeenCalled();
  expect(current).toHaveBeenCalledExactlyOnceWith({ kind: 'pause_sandbox', sandboxId: 'sbx_1' });
});
