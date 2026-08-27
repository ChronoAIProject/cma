import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ApiError, TransportError } from '../api/client';
import {
  BrowserCommandLedger,
  COMMAND_LEASE_MS,
  COMMAND_RETENTION_MS,
  CommandDescriptorMismatchError,
  CommandRequestUnavailableError,
  useIdempotentCommand,
} from './useIdempotentCommand';

const RESOURCE = 'launch:11111111-1111-4111-8111-111111111111';

function command(request: unknown = { expectedRevision: 7 }, persistRequest = true) {
  return {
    slot: 'launch:start:11111111-1111-4111-8111-111111111111',
    resource: RESOURCE,
    action: 'start',
    request,
    persistRequest,
  } as const;
}

function storageText(): string {
  return Array.from({ length: window.localStorage.length }, (_, index) => {
    const key = window.localStorage.key(index);
    return key === null ? '' : (window.localStorage.getItem(key) ?? '');
  }).join('\n');
}

function controlled<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe('durable browser command ledger', () => {
  it('retains the key, request, and original closure until a finite response', async () => {
    const ledger = new BrowserCommandLedger({ ownerId: 'owner-a' });
    const first = vi
      .fn<(key: string, request: unknown) => Promise<string>>()
      .mockRejectedValueOnce(new TransportError('response lost'))
      .mockResolvedValueOnce('landed');
    const changed = vi
      .fn<(key: string, request: unknown) => Promise<string>>()
      .mockResolvedValue('wrong closure');

    await expect(ledger.execute(command(), 'Launch start', first)).rejects.toBeInstanceOf(
      TransportError,
    );
    await expect(ledger.execute(command(), 'Launch start', changed)).resolves.toBe('landed');

    expect(first).toHaveBeenCalledTimes(2);
    expect(first.mock.calls[0]?.[0]).toBe(first.mock.calls[1]?.[0]);
    expect(first.mock.calls[1]?.[1]).toEqual({ expectedRevision: 7 });
    expect(changed).not.toHaveBeenCalled();
    expect(await ledger.has(RESOURCE)).toBe(false);
  });

  it('reattaches the exact persisted command after a page reload', async () => {
    const beforeReload = new BrowserCommandLedger({ ownerId: 'owner-a', instanceId: 'tab-a' });
    const first = vi.fn().mockRejectedValue(new TransportError('browser closed'));
    await expect(beforeReload.execute(command(), 'Launch start', first)).rejects.toBeInstanceOf(
      TransportError,
    );
    const originalKey = first.mock.calls[0]?.[0] as string;

    const afterReload = new BrowserCommandLedger({ ownerId: 'owner-a', instanceId: 'tab-b' });
    await expect(afterReload.pending([RESOURCE])).resolves.toMatchObject({
      slot: command().slot,
      request: { expectedRevision: 7 },
    });
    const replay = vi.fn().mockResolvedValue('same result');
    await expect(afterReload.execute(command(), 'Launch start', replay)).resolves.toBe(
      'same result',
    );
    expect(replay).toHaveBeenCalledWith(originalKey, { expectedRevision: 7 });
  });

  it('fails closed when a remounted caller offers a changed request body', async () => {
    const beforeReload = new BrowserCommandLedger({ ownerId: 'owner-a' });
    await expect(
      beforeReload.execute(command(), 'Launch start', () =>
        Promise.reject(new TransportError('response lost')),
      ),
    ).rejects.toBeInstanceOf(TransportError);

    const afterReload = new BrowserCommandLedger({ ownerId: 'owner-a' });
    const send = vi.fn().mockResolvedValue('must not send');
    await expect(
      afterReload.execute(command({ expectedRevision: 8 }), 'Launch start', send),
    ).rejects.toBeInstanceOf(CommandDescriptorMismatchError);
    expect(send).not.toHaveBeenCalled();
    expect(await afterReload.has(RESOURCE, 'start')).toBe(true);
  });

  it('keeps an in-flight server receipt unresolved and reuses its key', async () => {
    const ledger = new BrowserCommandLedger({ ownerId: 'owner-a' });
    const send = vi
      .fn()
      .mockRejectedValueOnce(
        new ApiError(409, 'the first request is still running', 'idempotency_in_flight'),
      )
      .mockResolvedValueOnce('settled');

    await expect(ledger.execute(command(), 'Launch start', send)).rejects.toMatchObject({
      code: 'idempotency_in_flight',
    });
    await expect(ledger.execute(command(), 'Launch start', send)).resolves.toBe('settled');
    expect(send.mock.calls[0]?.[0]).toBe(send.mock.calls[1]?.[0]);
  });

  it('stores only a fingerprint for a request containing private prompt text', async () => {
    const privateText = 'private issue details that must not enter browser storage';
    const ledger = new BrowserCommandLedger({ ownerId: 'owner-a' });
    await expect(
      ledger.execute(command({ prompt: privateText }, false), 'review update', () =>
        Promise.reject(new TransportError('response lost')),
      ),
    ).rejects.toBeInstanceOf(TransportError);

    expect(storageText()).not.toContain(privateText);
    expect((await ledger.pending([RESOURCE]))?.request).toBeUndefined();
  });

  it('refuses a changed private request after reload without inheriting its key', async () => {
    const original = new BrowserCommandLedger({ ownerId: 'owner-a' });
    await expect(
      original.execute(command({ prompt: 'original private text' }, false), 'review update', () =>
        Promise.reject(new TransportError('response lost')),
      ),
    ).rejects.toBeInstanceOf(TransportError);

    const reloaded = new BrowserCommandLedger({ ownerId: 'owner-a' });
    const send = vi.fn().mockResolvedValue('must not send');
    await expect(
      reloaded.execute(command({ prompt: 'different text' }, false), 'review update', send),
    ).rejects.toBeInstanceOf(CommandRequestUnavailableError);
    expect(send).not.toHaveBeenCalled();
  });

  it('expires and removes abandoned browser records at the server receipt bound', async () => {
    let now = 1_800_000_000_000;
    const first = new BrowserCommandLedger({ ownerId: 'owner-a', now: () => now });
    const send = vi.fn().mockRejectedValue(new TransportError('response lost'));
    await expect(first.execute(command(), 'Launch start', send)).rejects.toBeInstanceOf(
      TransportError,
    );
    const oldKey = send.mock.calls[0]?.[0] as string;

    now += COMMAND_RETENTION_MS + 1;
    const afterExpiry = new BrowserCommandLedger({ ownerId: 'owner-a', now: () => now });
    await expect(afterExpiry.pending([RESOURCE])).resolves.toBeNull();
    const next = vi.fn().mockResolvedValue('new intent');
    await afterExpiry.execute(command(), 'Launch start', next);
    expect(next.mock.calls[0]?.[0]).not.toBe(oldKey);
  });

  it('separates retained commands by opaque owner scope', async () => {
    const ownerA = new BrowserCommandLedger({ ownerId: 'owner-a' });
    await expect(
      ownerA.execute(command(), 'Launch start', () =>
        Promise.reject(new TransportError('response lost')),
      ),
    ).rejects.toBeInstanceOf(TransportError);

    const ownerB = new BrowserCommandLedger({ ownerId: 'owner-b' });
    await expect(ownerB.pending([RESOURCE])).resolves.toBeNull();
  });

  it('lets only one same-origin tab send while the exact command is active', async () => {
    let finish!: (value: string) => void;
    const firstSend = vi.fn(
      () =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    const secondSend = vi.fn().mockResolvedValue('duplicate');
    const tabA = new BrowserCommandLedger({ ownerId: 'owner-a', instanceId: 'tab-a' });
    const tabB = new BrowserCommandLedger({ ownerId: 'owner-a', instanceId: 'tab-b' });

    const first = tabA.execute(command(), 'Launch start', firstSend);
    await waitFor(() => expect(firstSend).toHaveBeenCalledOnce());
    await expect(tabB.execute(command(), 'Launch start', secondSend)).rejects.toBeInstanceOf(
      TransportError,
    );
    expect(secondSend).not.toHaveBeenCalled();

    finish('one result');
    await expect(first).resolves.toBe('one result');
  });

  it.each(['finite', 'unresolved'] as const)(
    'does not let a late %s result from an expired claim disturb its successor',
    async (outcome) => {
      let now = 1_800_000_000_000;
      const firstResult = controlled<string>();
      const successorResult = controlled<string>();
      const firstSend = vi
        .fn<(key: string, request: unknown) => Promise<string>>()
        .mockImplementation(() => firstResult.promise);
      const successorSend = vi
        .fn<(key: string, request: unknown) => Promise<string>>()
        .mockImplementation(() => successorResult.promise);
      const thirdSend = vi.fn().mockResolvedValue('must not send');
      const tabA = new BrowserCommandLedger({
        ownerId: 'owner-a',
        instanceId: 'tab-a',
        now: () => now,
      });
      const tabB = new BrowserCommandLedger({
        ownerId: 'owner-a',
        instanceId: 'tab-b',
        now: () => now,
      });
      const tabC = new BrowserCommandLedger({
        ownerId: 'owner-a',
        instanceId: 'tab-c',
        now: () => now,
      });

      const first = tabA.execute(command(), 'Launch start', firstSend);
      await waitFor(() => expect(firstSend).toHaveBeenCalledOnce());
      const originalKey = firstSend.mock.calls[0]?.[0];
      now += COMMAND_LEASE_MS + 1;

      const successor = tabB.execute(command(), 'Launch start', successorSend);
      await waitFor(() => expect(successorSend).toHaveBeenCalledOnce());
      expect(successorSend.mock.calls[0]?.[0]).toBe(originalKey);

      if (outcome === 'finite') {
        firstResult.resolve('late finite result');
        await expect(first).resolves.toBe('late finite result');
      } else {
        firstResult.reject(new TransportError('late unresolved result'));
        await expect(first).rejects.toBeInstanceOf(TransportError);
      }

      await expect(tabC.execute(command(), 'Launch start', thirdSend)).rejects.toBeInstanceOf(
        TransportError,
      );
      expect(thirdSend).not.toHaveBeenCalled();
      expect(await tabC.has(RESOURCE, 'start')).toBe(true);

      successorResult.resolve('successor settled');
      await expect(successor).resolves.toBe('successor settled');
      expect(await tabC.pending([RESOURCE])).toBeNull();
    },
  );

  it('keeps one claim when same-ledger callers join an active send', async () => {
    const result = controlled<string>();
    const originalSend = vi.fn(() => result.promise);
    const duplicateSend = vi.fn().mockResolvedValue('duplicate');
    let completedClaims = 0;
    const ledger = new BrowserCommandLedger({
      ownerId: 'owner-a',
      instanceId: 'tab-a',
      coordinator: {
        exclusive: async (_name, task) => {
          const value = await task();
          completedClaims += 1;
          return value;
        },
      },
    });

    const first = ledger.execute(command(), 'Launch start', originalSend);
    await waitFor(() => expect(originalSend).toHaveBeenCalledOnce());
    const duplicate = ledger.execute(command(), 'Launch start', duplicateSend);
    await waitFor(() => expect(completedClaims).toBe(2));
    expect(duplicateSend).not.toHaveBeenCalled();

    result.resolve('one result');
    await expect(Promise.all([first, duplicate])).resolves.toEqual(['one result', 'one result']);
    expect(originalSend).toHaveBeenCalledOnce();
    expect(duplicateSend).not.toHaveBeenCalled();
    expect(await ledger.pending([RESOURCE])).toBeNull();
  });

  it('keeps the original claim when a same-ledger caller joins after lease expiry', async () => {
    let now = 1_800_000_000_000;
    const result = controlled<string>();
    const originalSend = vi.fn(() => result.promise);
    const duplicateSend = vi.fn().mockResolvedValue('duplicate');
    let completedClaims = 0;
    const ledger = new BrowserCommandLedger({
      ownerId: 'owner-a',
      instanceId: 'tab-a',
      now: () => now,
      coordinator: {
        exclusive: async (_name, task) => {
          const value = await task();
          completedClaims += 1;
          return value;
        },
      },
    });

    const first = ledger.execute(command(), 'Launch start', originalSend);
    await waitFor(() => expect(originalSend).toHaveBeenCalledOnce());
    now += COMMAND_LEASE_MS + 1;

    const duplicate = ledger.execute(command(), 'Launch start', duplicateSend);
    await waitFor(() => expect(completedClaims).toBe(2));
    expect(duplicateSend).not.toHaveBeenCalled();

    result.resolve('one result');
    await expect(Promise.all([first, duplicate])).resolves.toEqual(['one result', 'one result']);
    expect(originalSend).toHaveBeenCalledOnce();
    expect(duplicateSend).not.toHaveBeenCalled();
    expect(await ledger.pending([RESOURCE])).toBeNull();
  });

  it('removes the execution lease when a send callback throws synchronously', async () => {
    const ledger = new BrowserCommandLedger({ ownerId: 'owner-a' });
    const failed = vi.fn(() => {
      throw new Error('synchronous request construction failed');
    });

    await expect(ledger.execute(command(), 'Launch start', failed)).rejects.toThrow(
      'synchronous request construction failed',
    );
    expect(await ledger.pending([RESOURCE])).toBeNull();

    const next = vi.fn().mockResolvedValue('submitted');
    await expect(ledger.execute(command(), 'Launch start', next)).resolves.toBe('submitted');
    expect(next).toHaveBeenCalledOnce();
  });
});

describe('idempotent command reconciliation hook', () => {
  it('replays once automatically and exposes the durable pending state after repeated loss', async () => {
    const send = vi
      .fn()
      .mockRejectedValueOnce(new TransportError('offline'))
      .mockRejectedValueOnce(new TransportError('still offline'))
      .mockResolvedValueOnce('landed');
    const { result } = renderHook(() => useIdempotentCommand('owner-a', [RESOURCE]));

    await act(async () => {
      await expect(result.current.execute(command(), 'Launch start', send)).rejects.toBeInstanceOf(
        TransportError,
      );
    });
    expect(send).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(result.current.pending?.slot).toBe(command().slot));

    await act(async () => {
      await expect(result.current.execute(command(), 'Launch start', send)).resolves.toBe('landed');
    });
    expect(send.mock.calls[0]?.[0]).toBe(send.mock.calls[2]?.[0]);
    expect(result.current.pending).toBeNull();
  });
});
