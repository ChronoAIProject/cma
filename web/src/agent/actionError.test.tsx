import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useAgentActionError } from './actionError';
import { usePresentedRetainedInputs } from './presentation';
import type { AgentCommand } from './controllerTypes';
import type { AgentV2Session } from './transport/client';
import { harness, json } from './transport/controller.fixture';
import { deferred, status } from './transport/queuedReconciliation.fixture';

beforeEach(() => sessionStorage.clear());

async function setup() {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const input = session.journal.retain('Retained message');
  const response = deferred<Response>();
  h.inputReceipt.mockReturnValueOnce(response.promise);
  const identity = vi.fn(() => false);
  const initialProps: { session?: AgentV2Session; pending?: AgentCommand } = {
    session,
    pending: 'send',
  };
  const view = renderHook(
    ({ session: current, pending }) => {
      const [error, setError, recovery] = useAgentActionError();
      const presented = usePresentedRetainedInputs(current, undefined, pending, identity, recovery);
      return { error, setError, recovery, presented };
    },
    { initialProps },
  );
  act(() => view.result.current.setError('Send failed', { session, key: input.key }));
  return {
    h,
    session,
    input,
    response,
    identity,
    view,
    async check() {
      view.rerender({ session });
      await waitFor(() => expect(h.inputReceipt).toHaveBeenCalledOnce());
    },
    async accept() {
      await act(async () => response.resolve(json(status(1, 'completed'))));
    },
  };
}

it('clears the matching error only through confirmed automatic acceptance', async () => {
  const f = await setup();
  await f.check();
  expect(f.view.result.current.error).toBe('Send failed');
  await f.accept();
  expect(f.view.result.current.error).toBeUndefined();
  expect(f.view.result.current.presented.pendingMessage).toBeUndefined();
  expect(f.view.result.current.presented.acceptedMessage).toBe(f.input.input);
  f.view.rerender({ session: f.session });
  expect(f.h.inputReceipt).toHaveBeenCalledOnce();
  expect(f.h.post).not.toHaveBeenCalled();
});

it.each(['different text', 'same text', 'same input'] as const)(
  'preserves a newer error with %s while the original receipt is in flight',
  async (replacement) => {
    const f = await setup();
    await f.check();
    const message = replacement === 'different text' ? 'Another action failed' : 'Send failed';
    act(() =>
      f.view.result.current.setError(
        message,
        replacement === 'same input' ? { session: f.session, key: f.input.key } : undefined,
      ),
    );
    await f.accept();
    expect(f.view.result.current.error).toBe(message);
    expect(f.session.journal.read()?.state).toBe('accepted');
    expect(f.h.inputReceipt).toHaveBeenCalledOnce();
    expect(f.h.post).not.toHaveBeenCalled();
  },
);

it.each(['absent', 'failed', 'identity changed'] as const)(
  'keeps the error when receipt lookup is %s',
  async (outcome) => {
    const f = await setup();
    await f.check();
    if (outcome === 'identity changed') f.h.currentUser.mockResolvedValue('bob');
    await act(async () =>
      f.response.resolve(
        outcome === 'failed'
          ? new Response('Unavailable', { status: 503 })
          : json(outcome === 'absent' ? null : status(1, 'completed')),
      ),
    );
    expect(f.view.result.current.error).toBe('Send failed');
    expect(f.view.result.current.presented.acceptedMessage).toBeUndefined();
    expect(f.h.post).not.toHaveBeenCalled();
    if (outcome === 'identity changed') expect(f.identity).toHaveBeenCalled();
  },
);

it.each(['unmount', 'session changed'] as const)(
  'ignores a late successful reply after %s',
  async (change) => {
    const f = await setup();
    await f.check();
    if (change === 'unmount') f.view.unmount();
    else {
      f.view.rerender({ session: undefined });
      act(() => f.view.result.current.setError('Current view error'));
    }
    await f.accept();
    expect(f.session.journal.read()?.state).toBe('pending');
    if (change !== 'unmount') expect(f.view.result.current.error).toBe('Current view error');
    expect(f.identity).not.toHaveBeenCalled();
    expect(f.h.post).not.toHaveBeenCalled();
  },
);

it.each(['other key', 'other session', 'no ownership', 'unreadable journal'] as const)(
  'does not clear an error with %s',
  async (mismatch) => {
    const f = await setup();
    if (mismatch === 'no ownership') act(() => f.view.result.current.setError('Send failed'));
    if (mismatch === 'unreadable journal') {
      const read = vi.spyOn(f.session.journal, 'read').mockImplementation(() => {
        throw new Error('Unavailable journal');
      });
      act(() =>
        f.view.result.current.setError('Send failed', { session: f.session, key: f.input.key }),
      );
      read.mockRestore();
    }
    const accepted = f.view.result.current.recovery(
      mismatch === 'other session' ? f.session.forSnapshotRefresh() : f.session,
      mismatch === 'other key' ? 'unrelated-key' : f.input.key,
    );
    f.session.journal.accepted(f.input.key, { commandId: 'cmd_1', responseId: 'resp_1' });
    act(accepted);
    expect(f.view.result.current.error).toBe('Send failed');
    expect(f.h.inputReceipt).not.toHaveBeenCalled();
    expect(f.h.post).not.toHaveBeenCalled();
  },
);

it('does not attribute an authorization failure to a previous retained input', async () => {
  const f = await setup();
  const retained = vi.fn();
  f.h.currentUser.mockRejectedValueOnce(new TypeError('Authorization unavailable'));
  await expect(
    f.session.send(f.input.input, vi.fn(), undefined, undefined, undefined, retained),
  ).rejects.toThrow();
  expect(retained).not.toHaveBeenCalled();
  expect(f.session.journal.read()).toEqual(f.input);
  expect(f.h.post).not.toHaveBeenCalled();
});

it('does not clear the old error if another request replaces the journal during authorization', async () => {
  const f = await setup();
  const identity = deferred<string>();
  f.h.currentUser.mockReturnValueOnce(identity.promise);
  f.view.rerender({ session: f.session });
  f.session.journal.refused(f.input.key);
  const replacement = f.session.journal.retain('A different request');
  await act(async () => identity.resolve('alice'));
  await waitFor(() => expect(f.h.inputReceipt).toHaveBeenCalledOnce());
  await f.accept();
  expect(f.session.journal.read()?.key).toBe(replacement.key);
  expect(f.session.journal.read()?.state).toBe('accepted');
  expect(f.view.result.current.error).toBe('Send failed');
  expect(f.h.post).not.toHaveBeenCalled();
});
