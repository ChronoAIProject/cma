import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { json } from './controller.fixture';
import { reconciliationHarness, status } from './queuedReconciliation.fixture';
import { useQueuedInputs } from './queuedInputsStatus';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => vi.useRealTimers());

const identity = () => false;
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

it('retains an uncertain edit when another queued command reaches a terminal state', async () => {
  const { session, journal, command, fetch, setRows } = reconciliationHarness();
  const edit = journal.queuedInputs.retain({
    key: 'edit-key',
    responseId: 'resp_1',
    commandId: 'cmd_1',
    revision: 7,
    original: 'Message 1',
    action: 'edit',
    text: 'Keep the revised message\n保留原文',
  });
  setRows([1, 2]);
  const hook = renderHook(() => useQueuedInputs(session, true, true, '0', identity));

  await tick();
  expect(hook.result.current.items).toMatchObject([
    { commandId: 'cmd_1', needsRecovery: true },
    { commandId: 'cmd_2', needsRecovery: false },
  ]);

  setRows([1]);
  command.mockResolvedValueOnce(json(status(1, 'cancelled')));
  await tick(2000);
  expect(command).toHaveBeenCalledWith(2);
  expect(hook.result.current.outcomes).toMatchObject([
    { commandId: 'cmd_2', terminal: false, kind: 'unknown' },
  ]);
  expect(journal.queuedInputs.read()).toEqual([edit]);

  command.mockImplementation(async (id) => json(status(id, 'cancelled')));
  await tick(10000);
  expect(hook.result.current.outcomes).toMatchObject([
    { commandId: 'cmd_2', responseId: 'resp_2', terminal: true, kind: 'cancelled' },
  ]);
  expect(hook.result.current.items).toMatchObject([
    { commandId: 'cmd_1', responseId: 'resp_1', needsRecovery: true },
  ]);
  expect(journal.queuedInputs.read()).toEqual([edit]);
  expect(fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toEqual([]);
});
