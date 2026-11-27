import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AgentV2Session } from './client';
import { harness, json, queuedPage } from './controller.fixture';
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
const unavailable = (status: number) =>
  json(
    { error: { type: 'server_error', code: 'unavailable', message: 'Unavailable', param: null } },
    status,
  );
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

it.each(['hint', 'focus', 'visibility', 'refresh'])(
  'recovers queue discovery after a compatibility 404 through %s without remounting',
  async (trigger) => {
    const h = fixture();
    h.listQueued.mockImplementationOnce(async () => unavailable(404));
    const hook = renderHook(
      ({ revision }) => useQueuedInputs(h.session, true, true, revision, unchangedIdentity),
      { initialProps: { revision: '0' } },
    );
    await advance(1000);
    expect(h.listQueued).toHaveBeenCalledTimes(1);
    expect(hook.result.current.error).toBeUndefined();
    h.listQueued.mockImplementation(async () => json(queuedPage('Discovered after deployment')));
    if (trigger === 'hint') hook.rerender({ revision: '1' });
    else if (trigger === 'refresh') act(() => hook.result.current.refresh());
    else if (trigger === 'focus') act(() => window.dispatchEvent(new Event('focus')));
    else {
      const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
      act(() => document.dispatchEvent(new Event('visibilitychange')));
      visibility.mockReturnValue('visible');
      act(() => document.dispatchEvent(new Event('visibilitychange')));
    }
    await advance(trigger === 'refresh' ? 100 : 1000);
    expect(h.listQueued).toHaveBeenCalledTimes(2);
    expect(hook.result.current.items[0]?.input).toBe('Discovered after deployment');
    expect(h.currentUser).toHaveBeenCalledTimes(3);
    expect(h.post).not.toHaveBeenCalled();
    hook.unmount();
  },
);

it.each(['turn', 'request'])(
  'retains a compatibility retry when an early %s refresh precedes the only stream hint',
  async (trigger) => {
    const h = fixture();
    h.listQueued.mockImplementationOnce(async () => unavailable(404));
    const hook = renderHook(
      ({ revision, active, request }) =>
        useQueuedInputs(h.session, true, active, revision, unchangedIdentity, request),
      { initialProps: { revision: '0', active: false, request: 0 } },
    );
    await advance(0);
    expect(h.listQueued).toHaveBeenCalledTimes(1);
    h.listQueued.mockImplementation(async () => json(queuedPage('Discovered after deployment')));
    hook.rerender({
      revision: '0',
      active: trigger === 'turn',
      request: trigger === 'request' ? 1 : 0,
    });
    await advance(50);
    hook.rerender({
      revision: '1',
      active: trigger === 'turn',
      request: trigger === 'request' ? 1 : 0,
    });
    await advance(4950);
    expect(h.listQueued.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(h.listQueued.mock.calls.length).toBeLessThanOrEqual(3);
    expect(hook.result.current.items[0]?.input).toBe('Discovered after deployment');
    expect(h.post).not.toHaveBeenCalled();
    hook.unmount();
  },
);

it.each(['hint', 'refresh'])(
  'retains a queue and real failure through a compatibility 404, then recovers on %s',
  async (trigger) => {
    const h = fixture();
    h.listQueued
      .mockImplementationOnce(async () => json(queuedPage('Retained message')))
      .mockImplementationOnce(async () => unavailable(503))
      .mockImplementationOnce(async () => unavailable(404))
      .mockImplementation(async () => json(queuedPage('Current message')));
    const hook = renderHook(
      ({ revision }) => useQueuedInputs(h.session, true, true, revision, unchangedIdentity),
      { initialProps: { revision: '0' } },
    );
    await advance(6000);
    expect(h.listQueued).toHaveBeenCalledTimes(3);
    expect(hook.result.current.items[0]?.input).toBe('Retained message');
    expect(hook.result.current.error).toContain('could not be refreshed');
    await advance(29000);
    expect(h.listQueued).toHaveBeenCalledTimes(3);
    if (trigger === 'hint') hook.rerender({ revision: '1' });
    else act(() => hook.result.current.refresh());
    await advance(trigger === 'refresh' ? 100 : 0);
    expect(h.listQueued).toHaveBeenCalledTimes(4);
    expect(hook.result.current.items[0]?.input).toBe('Current message');
    expect(hook.result.current.error).toBeUndefined();
    expect(h.currentUser).toHaveBeenCalledTimes(6);
    expect(h.post).not.toHaveBeenCalled();
    hook.unmount();
  },
);

it('recovers a compatibility 404 on a slow timer without a stream event or remount', async () => {
  const h = fixture();
  h.listQueued.mockImplementationOnce(async () => unavailable(404));
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', unchangedIdentity));
  await advance(29999);
  expect(h.listQueued).toHaveBeenCalledTimes(1);
  await advance(1);
  expect(h.listQueued).toHaveBeenCalledTimes(2);
  expect(hook.result.current.error).toBeUndefined();
  hook.unmount();
});

it('uses a slow fallback for an empty active queue and stays idle after the turn ends', async () => {
  const h = fixture();
  const hook = renderHook(
    ({ active }) => useQueuedInputs(h.session, true, active, '0', unchangedIdentity),
    { initialProps: { active: true } },
  );
  await advance(60000);
  expect(h.listQueued).toHaveBeenCalledTimes(7);
  expect(h.currentUser).toHaveBeenCalledTimes(14);
  hook.rerender({ active: false });
  await advance(100);
  const count = h.listQueued.mock.calls.length;
  await advance(60000);
  expect(h.listQueued).toHaveBeenCalledTimes(count);
  hook.unmount();
});

it('keeps the empty queue cadence under stream hints and refreshes after submission', async () => {
  const h = fixture();
  const hook = renderHook(
    ({ revision, request }) =>
      useQueuedInputs(h.session, true, true, revision, unchangedIdentity, request),
    { initialProps: { revision: '0', request: 0 } },
  );
  await advance(0);
  for (let n = 1; n <= 300; n++) {
    hook.rerender({ revision: String(n), request: 0 });
    await advance(200);
  }
  expect(h.listQueued).toHaveBeenCalledTimes(7);
  expect(h.currentUser).toHaveBeenCalledTimes(14);
  h.listQueued.mockImplementation(async () => json(queuedPage('New queued message')));
  hook.rerender({ revision: '300', request: 1 });
  await advance(100);
  expect(h.listQueued).toHaveBeenCalledTimes(8);
  expect(hook.result.current.items[0]?.input).toBe('New queued message');
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
