import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { agentClient } from '../test/agent';
import {
  emptyTaskDraft,
  openTaskDraftStore,
  taskDraftStorageKey,
} from '../create/taskDraftStorage';
import { lifecycleStorageKey } from '../create/lifecycleIntentStorage';
import { retainTaskViewer } from '../create/taskIdentity';
import { OriginalRequestReuse } from './OriginalRequestReuse';

const identity = {
  viewerId: 'owner',
  authEpoch: 1,
  agentId: 'agt_1',
  sandboxId: 'sbx_1',
  workspaceId: 'wsp_1',
  worktreeId: 'wkt_1',
};
const text = 'Exact admitted request\n请求 🧭';
const original = {
  kind: 'ready' as const,
  agentId: 'agt_1',
  text,
  source: 'admitted-first-task' as const,
  turnSequence: null,
};
const start = () =>
  fireEvent.click(screen.getByRole('button', { name: 'Start a new agent with this request' }));
beforeEach(() => {
  sessionStorage.clear();
  retainTaskViewer('owner');
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});
function fixture(read = vi.fn().mockResolvedValue(original)) {
  const create = vi.fn();
  const send = vi.fn();
  const client = agentClient({
    readAgentOriginalRequest: read,
    createAgentLifecycle: create,
    runAgent: send,
  });
  const onImported = vi.fn();
  return { read, create, send, client, onImported };
}
function seed() {
  const store = openTaskDraftStore('owner');
  store.read();
  store.save({ ...emptyTaskDraft, text: 'Independent draft', revision: 'original' });
}

it('does no mount read or creation and saves exact source text before navigating', async () => {
  const f = fixture();
  render(<OriginalRequestReuse {...f} identity={identity} />);
  await act(async () => {});
  expect(f.read).not.toHaveBeenCalled();
  start();
  await waitFor(() => expect(f.onImported).toHaveBeenCalledOnce());
  expect(openTaskDraftStore('owner').read().draft).toMatchObject({
    text,
    repositoryUrl: '',
    repositoryRef: '',
    machineIncluded: false,
  });
  expect(f.read).toHaveBeenCalledExactlyOnceWith('agt_1', { signal: expect.any(AbortSignal) });
  expect(f.create).not.toHaveBeenCalled();
  expect(f.send).not.toHaveBeenCalled();
});

it('keeps a dirty Home draft on refusal and preserves it in recovery after explicit replacement', async () => {
  seed();
  const f = fixture();
  render(<OriginalRequestReuse {...f} identity={identity} />);
  start();
  expect(await screen.findByRole('dialog', { name: 'Use this request in Home?' })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Keep current draft' }));
  expect(openTaskDraftStore('owner').read().draft.text).toBe('Independent draft');
  expect(f.onImported).not.toHaveBeenCalled();
  start();
  await screen.findByRole('button', { name: 'Use this request' });
  fireEvent.click(screen.getByRole('button', { name: 'Use this request' }));
  expect(f.onImported).toHaveBeenCalledOnce();
  const value = openTaskDraftStore('owner').read();
  expect(value.draft.text).toBe(text);
  expect(value.recoverable).toEqual([
    expect.objectContaining({
      draft: expect.objectContaining({ text: 'Independent draft', revision: 'original' }),
    }),
  ]);
  expect(f.create).not.toHaveBeenCalled();
  expect(f.send).not.toHaveBeenCalled();
});

it('preserves UNKNOWN responsibility and keeps the source visible for copy without navigation', async () => {
  const key = lifecycleStorageKey('owner');
  const request = JSON.stringify({
    version: 1,
    intent: {
      key: 'original-key',
      body: { name: 'Original', initial_message: 'Original pending body' },
    },
  });
  sessionStorage.setItem(key, request);
  const f = fixture();
  render(<OriginalRequestReuse {...f} identity={identity} />);
  start();
  expect(await screen.findByText(/Resolve the original pending task request/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Copy request' })).toBeVisible();
  expect(sessionStorage.getItem(key)).toBe(request);
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
  expect(f.onImported).not.toHaveBeenCalled();
  expect(f.create).not.toHaveBeenCalled();
});

it('does not overwrite a newer draft after a confirmation was offered', async () => {
  seed();
  const f = fixture();
  render(<OriginalRequestReuse {...f} identity={identity} />);
  start();
  await screen.findByRole('dialog');
  const store = openTaskDraftStore('owner');
  store.read();
  store.save({ ...emptyTaskDraft, text: 'Later edit', revision: 'later' });
  fireEvent.click(screen.getByRole('button', { name: 'Use this request' }));
  expect(await screen.findByText(/Another saved draft changed/)).toBeVisible();
  expect(openTaskDraftStore('owner').read().draft.text).toBe('Later edit');
  expect(f.onImported).not.toHaveBeenCalled();
});

it.each(['viewerId', 'authEpoch', 'agentId', 'client', 'cancel', 'unmount'] as const)(
  'ignores a late source after %s changes',
  async (change) => {
    let resolve!: (value: typeof original) => void;
    const read = vi.fn().mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const f = fixture(read);
    const view = render(<OriginalRequestReuse {...f} identity={identity} />);
    start();
    await waitFor(() => expect(read).toHaveBeenCalledOnce());
    const signal = read.mock.calls[0]![1].signal as AbortSignal;
    if (change === 'unmount') view.unmount();
    else if (change === 'cancel') fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    else
      view.rerender(
        <OriginalRequestReuse
          {...f}
          client={change === 'client' ? agentClient() : f.client}
          identity={
            change === 'client'
              ? identity
              : { ...identity, [change]: change === 'authEpoch' ? 2 : 'other' }
          }
        />,
      );
    expect(signal.aborted).toBe(true);
    await act(async () => resolve(original));
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
    expect(f.onImported).not.toHaveBeenCalled();
    expect(f.create).not.toHaveBeenCalled();
    expect(read).toHaveBeenCalledOnce();
  },
);

it.each(['unavailable', 'old-server', 'wrong-agent', 'forbidden'] as const)(
  'does not guess an original request for %s',
  async (kind) => {
    const read =
      kind === 'unavailable'
        ? vi.fn().mockResolvedValue({ kind: 'unavailable', agentId: 'agt_1' })
        : kind === 'old-server'
          ? vi.fn().mockRejectedValue(new ApiError(404, 'Unknown route'))
          : kind === 'forbidden'
            ? vi.fn().mockRejectedValue(new ApiError(403, 'History access denied'))
            : vi.fn().mockResolvedValue({ ...original, agentId: 'other' });
    const f = fixture(read);
    render(<OriginalRequestReuse {...f} identity={identity} />);
    start();
    expect(
      await screen.findByText(
        kind === 'forbidden'
          ? 'History access denied'
          : kind === 'wrong-agent'
            ? 'The original request did not match this Agent.'
            : 'Original request unavailable',
      ),
    ).toBeVisible();
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
    expect(f.onImported).not.toHaveBeenCalled();
    expect(f.create).not.toHaveBeenCalled();
  },
);

it('retains source and current draft after storage failure and never navigates on partial success', async () => {
  seed();
  const f = fixture();
  render(<OriginalRequestReuse {...f} identity={identity} />);
  start();
  await screen.findByRole('dialog');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Storage full');
  });
  fireEvent.click(screen.getByRole('button', { name: 'Use this request' }));
  expect(await screen.findByText('Storage full')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Copy request' })).toBeVisible();
  expect(openTaskDraftStore('owner').read().draft.text).toBe('Independent draft');
  expect(f.onImported).not.toHaveBeenCalled();
});

it('honors a rate limit without automatically reading again', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const read = vi
    .fn()
    .mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 20_000))
    .mockResolvedValue(original);
  const f = fixture(read);
  render(<OriginalRequestReuse {...f} identity={identity} />);
  start();
  await act(async () => {});
  expect(
    screen.getByRole('button', { name: 'Start a new agent with this request' }),
  ).toBeDisabled();
  await act(() => vi.advanceTimersByTimeAsync(20_000));
  expect(read).toHaveBeenCalledOnce();
  start();
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(2);
  expect(f.onImported).toHaveBeenCalledOnce();
});
