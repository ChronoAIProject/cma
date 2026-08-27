import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { TransportError } from '../api/client';
import { BrowserCommandLedger, useIdempotentCommand } from './useIdempotentCommand';

describe('browser command ledger', () => {
  it('retains the key and original request closure until a finite response', async () => {
    const ledger = new BrowserCommandLedger();
    const first = vi
      .fn<(key: string) => Promise<string>>()
      .mockRejectedValueOnce(new TransportError('response lost'))
      .mockResolvedValueOnce('landed');
    const changed = vi.fn<(key: string) => Promise<string>>().mockResolvedValue('wrong body');

    await expect(ledger.execute('save', first)).rejects.toBeInstanceOf(TransportError);
    await expect(ledger.execute('save', changed)).resolves.toBe('landed');

    expect(first).toHaveBeenCalledTimes(2);
    expect(first.mock.calls[0]?.[0]).toBe(first.mock.calls[1]?.[0]);
    expect(changed).not.toHaveBeenCalled();
    expect(ledger.has('save')).toBe(false);
  });

  it('joins duplicate clicks to one in-flight request', async () => {
    let resolve!: (value: string) => void;
    const send = vi.fn(
      () =>
        new Promise<string>((done) => {
          resolve = done;
        }),
    );
    const ledger = new BrowserCommandLedger();

    const first = ledger.execute('publish', send);
    const duplicate = ledger.execute('publish', send);
    expect(send).toHaveBeenCalledOnce();
    resolve('one result');
    await expect(Promise.all([first, duplicate])).resolves.toEqual(['one result', 'one result']);
  });
});

describe('idempotent command reconciliation', () => {
  it('replays once automatically and exposes a pending state after repeated transport loss', async () => {
    const send = vi
      .fn()
      .mockRejectedValueOnce(new TransportError('offline'))
      .mockRejectedValueOnce(new TransportError('still offline'))
      .mockResolvedValueOnce('landed');
    const { result } = renderHook(() => useIdempotentCommand());

    await act(async () => {
      await expect(
        result.current.execute('cancel', 'Launch cancellation', send),
      ).rejects.toBeInstanceOf(TransportError);
    });

    expect(send).toHaveBeenCalledTimes(2);
    await waitFor(() =>
      expect(result.current.pending).toEqual({ slot: 'cancel', label: 'Launch cancellation' }),
    );

    await act(async () => {
      await expect(result.current.reconcile('cancel', 'Launch cancellation')).resolves.toBe(
        'landed',
      );
    });
    expect(send.mock.calls[0]?.[0]).toBe(send.mock.calls[2]?.[0]);
    expect(result.current.pending).toBeNull();
  });
});
