import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useAgentController } from '../controller';
import { accepted, at, binding, cursor, harness, item, json, page } from './controller.fixture';

beforeEach(() => sessionStorage.clear());

it.each(['pending', 'accepted'] as const)(
  'retains a %s v2 input across access suspension and resumes observation without resubmitting',
  async (admission) => {
    const h = harness();
    if (admission === 'pending')
      h.post.mockImplementationOnce(
        (request) =>
          new Promise((_resolve, reject) => {
            request!.signal!.addEventListener(
              'abort',
              () => reject(new DOMException('Aborted', 'AbortError')),
              { once: true },
            );
          }),
      );
    const { result, rerender, unmount } = renderHook(
      ({ suspended }) => useAgentController(h.client, 'agt_1', { suspended }),
      { initialProps: { suspended: false } },
    );
    await waitFor(() => expect(h.streaming && result.current.inputAvailability.allowed).toBe(true));
    const onAccepted = vi.fn();
    let sent!: Promise<void>;
    act(() => {
      sent = result.current.send('Exact retained input', onAccepted);
    });
    await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
    if (admission === 'accepted') await act(() => sent);
    await act(async () => h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding));
    const retained = result.current.state;
    rerender({ suspended: true });
    await act(() => sent);
    expect(result.current.state).toBe(retained);
    expect(result.current.inputAvailability.allowed).toBe(false);
    expect(result.current.recoverPending).toBeUndefined();

    rerender({ suspended: false });
    const streams = () => h.fetch.mock.calls.filter(([url]) => String(url).includes('/events?'));
    await waitFor(() => expect(streams()).toHaveLength(2));
    expect(
      new URL(String(streams()[1]![0]), 'https://cma.example').searchParams.get('after_cursor'),
    ).toBe(cursor(1));
    expect(h.post).toHaveBeenCalledOnce();
    if (admission === 'pending') {
      expect(onAccepted).not.toHaveBeenCalled();
      expect(result.current.pendingMessage).toBe('Exact retained input');
      await act(() => result.current.recoverPending!());
      expect(h.post).toHaveBeenCalledTimes(2);
      const [original, recovered] = h.post.mock.calls.map(([request]) => request!);
      expect(recovered!.body).toBe(original!.body);
      expect(new Headers(recovered!.headers).get('idempotency-key')).toBe(
        new Headers(original!.headers).get('idempotency-key'),
      );
    } else {
      expect(onAccepted).toHaveBeenCalledOnce();
      expect(result.current.acceptedMessage).toBe('Exact retained input');
      await act(() => result.current.recoverPending!());
      expect(h.post).toHaveBeenCalledOnce();
    }
    await act(async () =>
      h.durable(2, { type: 'message', role: 'assistant', content: 'Continued accepted turn' }),
    );
    expect(result.current.state).toMatchObject({
      conversation: { messages: [expect.objectContaining({ content: 'Continued accepted turn' })] },
    });
    expect(h.client.runAgent).not.toHaveBeenCalled();
    unmount();
  },
);

it('keeps rich observation alive during a pending input and settles only on the receipt', async () => {
  const h = harness();
  let resolve!: (response: Response) => void;
  h.post.mockReturnValueOnce(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming && result.current.inputAvailability.allowed).toBe(true));
  const onAccepted = vi.fn();
  let sent!: Promise<void>;
  act(() => {
    sent = result.current.send('Inspect files', onAccepted);
  });
  await waitFor(() => expect(h.post).toHaveBeenCalledOnce());
  await act(async () => {
    h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    h.durable(2, {
      type: 'tool_started',
      call_id: 'read',
      title: 'Read files',
      kind: 'read',
      status: 'running',
    });
    h.live('reasoning_summary_delta', 'Checking the files', 2);
  });
  expect(result.current.state).toMatchObject({
    status: 'ready',
    conversation: {
      activeRunId: '1',
      activeRunStartedAt: at,
      toolCalls: [expect.objectContaining({ id: 'read', status: 'running' })],
      reasoning: [expect.objectContaining({ content: 'Checking the files' })],
    },
  });
  expect(result.current.pending).toBe('send');
  expect(onAccepted).not.toHaveBeenCalled();
  expect(h.cancel).not.toHaveBeenCalled();
  await act(async () => {
    resolve(accepted());
    await sent;
  });
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(result.current.pending).toBeUndefined();
  expect(h.cancel).not.toHaveBeenCalled();
  await act(async () => {
    h.durable(3, { type: 'tool_updated', call_id: 'read', status: 'completed', title: null });
    h.live('text_delta', 'The files are ready.', 3);
    h.durable(4, { type: 'message', role: 'assistant', content: 'The files are ready.' });
    h.durable(5, { type: 'turn_completed', turn_sequence: 1 });
  });
  expect(result.current.state).toMatchObject({
    status: 'ready',
    conversation: {
      cursor: 5,
      activeRunId: undefined,
      reasoning: [],
      messages: [expect.objectContaining({ content: 'The files are ready.', status: 'complete' })],
      toolCalls: [expect.objectContaining({ id: 'read', status: 'completed' })],
    },
  });
  expect(h.client.runAgent).not.toHaveBeenCalled();
  expect(h.client.replayAgent).not.toHaveBeenCalled();
  unmount();
});

it('recovers an uncertain input across controller reload using the same request body and key', async () => {
  const h = harness();
  h.post.mockRejectedValueOnce(new TypeError('connection lost'));
  const first = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(first.result.current.inputAvailability.allowed).toBe(true));
  await act(() => first.result.current.send('Exact original message'));
  expect(first.result.current.pendingMessage).toBe('Exact original message');
  first.unmount();
  const second = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(second.result.current.state.status).toBe('ready'));
  expect(second.result.current.pendingMessage).toBe('Exact original message');
  await act(() => second.result.current.recoverPending!());
  expect(h.post).toHaveBeenCalledTimes(2);
  const original = h.post.mock.calls[0]![0]!;
  const recovered = h.post.mock.calls[1]![0]!;
  expect(recovered.body).toBe(original.body);
  expect(new Headers(recovered.headers).get('idempotency-key')).toBe(
    new Headers(original.headers).get('idempotency-key'),
  );
  expect(second.result.current.pendingMessage).toBeUndefined();
  second.unmount();
});

it('waits for every snapshot page before tailing, preserving legacy turn control unavailability', async () => {
  const h = harness();
  h.snapshot.mockResolvedValueOnce(
    json(page([item(1, { type: 'turn_started', turn_sequence: 1 })], 2, 1)),
  );
  let finish!: (response: Response) => void;
  h.snapshot.mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledTimes(2));
  expect(h.streaming).toBe(false);
  await act(async () =>
    finish(
      json(
        page(
          [
            item(2, {
              type: 'tool_started',
              call_id: 'legacy',
              title: 'Legacy call',
              kind: 'read',
              status: 'completed',
            }),
          ],
          2,
        ),
      ),
    ),
  );
  await waitFor(() => expect(h.streaming).toBe(true));
  expect(result.current.controlsUnavailable).toBe(true);
  expect(result.current.state).toMatchObject({
    status: 'ready',
    conversation: {
      activeRunStartedAt: at,
      toolCalls: [expect.objectContaining({ id: 'legacy', status: 'completed' })],
    },
  });
  await act(() => result.current.interrupt());
  expect(result.current.actionError).toContain('does not support response controls');
  expect(h.client.interruptAgent).not.toHaveBeenCalled();
  unmount();
});
