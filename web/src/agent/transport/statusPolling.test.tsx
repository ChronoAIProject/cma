import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AgentV2Session } from './client';
import { harness, json } from './controller.fixture';
import { agentTransportJournal } from './journal';
import { useQueuedInputs } from './queuedInputsStatus';
import { useControlStatus } from './controlStatus';
import { commandStatus } from './controls.fixture';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const unchangedIdentity = () => false;
function fixture() {
  const h = harness();
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  const session = new AgentV2Session(h.sdk, journal, 'alice', {
    storage: sessionStorage,
    currentUser: h.currentUser,
  });
  return { ...h, journal, session };
}

it('bounds stream-driven queue reads while preserving both fresh authorization checks', async () => {
  const h = fixture();
  const hook = renderHook(
    ({ revision }) => useQueuedInputs(h.session, true, true, revision, unchangedIdentity),
    { initialProps: { revision: '0' } },
  );
  await advance(0);
  for (let n = 1; n <= 300; n++) {
    hook.rerender({ revision: String(n) });
    await advance(200);
  }
  expect(h.listQueued.mock.calls.length).toBeGreaterThanOrEqual(30);
  expect(h.listQueued.mock.calls.length).toBeLessThanOrEqual(31);
  expect(h.currentUser).toHaveBeenCalledTimes(2 * h.listQueued.mock.calls.length);
  expect(h.post).not.toHaveBeenCalled();
  hook.unmount();
});

it('keeps error backoff and an honest error while events arrive, then recovers', async () => {
  const h = fixture();
  h.listQueued.mockImplementation(async () =>
    json(
      {
        error: { type: 'server_error', code: 'unavailable', message: 'Busy', param: null },
      },
      503,
    ),
  );
  const hook = renderHook(
    ({ revision }) => useQueuedInputs(h.session, true, true, revision, unchangedIdentity),
    { initialProps: { revision: '0' } },
  );
  await advance(0);
  for (let n = 1; n <= 100; n++) {
    hook.rerender({ revision: String(n) });
    await advance(200);
  }
  expect(h.listQueued).toHaveBeenCalledTimes(4);
  expect(hook.result.current.error).toContain('could not be refreshed');
  h.listQueued.mockImplementation(async () =>
    json({
      object: 'cma.agent.queued_inputs',
      agent_id: 'agt_1',
      data: [],
      next_after: null,
    }),
  );
  await advance(4000);
  expect(hook.result.current.error).toBeUndefined();
  hook.unmount();
});

it('never publishes queue data when the post-read identity check fails', async () => {
  const h = fixture();
  h.currentUser.mockResolvedValueOnce('alice').mockResolvedValue('another-viewer');
  const changed = vi.fn(() => true);
  const hook = renderHook(
    ({ revision }) => useQueuedInputs(h.session, true, true, revision, changed),
    { initialProps: { revision: '0' } },
  );
  await advance(0);
  hook.rerender({ revision: '1' });
  await advance(60000);
  expect(h.listQueued).toHaveBeenCalledTimes(1);
  expect(changed).toHaveBeenCalledTimes(1);
  expect(hook.result.current.items).toEqual([]);
  hook.unmount();
});

function retainControl(h: ReturnType<typeof fixture>) {
  h.journal.retainControl({
    key: 'control-key',
    responseId: 'resp_1',
    commandId: null,
    body: { command: { type: 'cancel' }, turn_sequence: 1, expected_agent_revision: 10 },
  });
  h.journal.acceptedControl('control-key', 'cmd_control');
  h.command.mockImplementation(async () => json(commandStatus(null)));
}

it('backs off an unchanged action, pauses while hidden and promptly observes completion on return', async () => {
  const h = fixture();
  retainControl(h);
  let visible = true;
  vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() =>
    visible ? 'visible' : 'hidden',
  );
  const hook = renderHook(() => useControlStatus(h.session, unchangedIdentity));
  await advance(60000);
  expect(h.command.mock.calls.length).toBeLessThanOrEqual(10);
  expect(h.currentUser).toHaveBeenCalledTimes(2 * h.command.mock.calls.length);
  visible = false;
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  const count = h.command.mock.calls.length;
  await advance(60000);
  expect(h.command).toHaveBeenCalledTimes(count);
  h.command.mockImplementation(async () => json(commandStatus(null, 1, true)));
  visible = true;
  act(() => {
    document.dispatchEvent(new Event('visibilitychange'));
    window.dispatchEvent(new Event('focus'));
  });
  await advance(0);
  expect(h.command).toHaveBeenCalledTimes(count + 1);
  expect(hook.result.current).toBeUndefined();
  await advance(60000);
  expect(h.command).toHaveBeenCalledTimes(count + 1);
  expect(h.control).not.toHaveBeenCalled();
  hook.unmount();
});

it('does not retry a control with the wrong target or hide its error', async () => {
  const h = fixture();
  retainControl(h);
  h.command.mockImplementation(async () => json(commandStatus(null, 2)));
  const hook = renderHook(() => useControlStatus(h.session, unchangedIdentity));
  await advance(60000);
  expect(h.command).toHaveBeenCalledTimes(1);
  expect(hook.result.current).toContain('status is unavailable');
  hook.unmount();
});
