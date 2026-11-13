import { expect, it, vi } from 'vitest';
import { createProjectionPublication } from './publication';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

it('publishes completed projections with one fresh check after their metadata finishes', async () => {
  const metadata = deferred<string>();
  const authorize = vi.fn(async () => undefined);
  const guard = vi.fn();
  const publish = createProjectionPublication(
    metadata.promise,
    authorize,
    guard,
    new AbortController().signal,
  );
  const first = vi.fn();
  const second = vi.fn();
  const pending = [publish(first), publish(second)];
  await Promise.resolve();
  expect(authorize).not.toHaveBeenCalled();
  metadata.resolve('exact-agent');
  await Promise.all(pending);
  expect(authorize).toHaveBeenCalledOnce();
  expect(first).toHaveBeenCalledWith('exact-agent');
  expect(second).toHaveBeenCalledWith('exact-agent');
  expect(guard).toHaveBeenCalledTimes(2);
  await publish(vi.fn());
  expect(authorize).toHaveBeenCalledTimes(2);
  publish.dispose();
});

it('gives data arriving during an identity read a later fresh check', async () => {
  const proof = deferred<void>();
  const laterProof = deferred<void>();
  const authorize = vi
    .fn()
    .mockReturnValueOnce(proof.promise)
    .mockReturnValueOnce(laterProof.promise);
  const publish = createProjectionPublication(
    Promise.resolve('agent'),
    authorize,
    vi.fn(),
    new AbortController().signal,
  );
  const first = vi.fn();
  const second = vi.fn();
  const firstResult = publish(first);
  await Promise.resolve();
  await Promise.resolve();
  expect(authorize).toHaveBeenCalledOnce();
  const secondResult = publish(second);
  proof.resolve();
  await firstResult;
  await Promise.resolve();
  expect(second).not.toHaveBeenCalled();
  expect(authorize).toHaveBeenCalledTimes(2);
  laterProof.resolve();
  await secondResult;
  expect(second).toHaveBeenCalledOnce();
  publish.dispose();
});

it('rejects cancellation before metadata without waiting for an abort-resistant read', async () => {
  const metadata = deferred<string>();
  const request = new AbortController();
  const authorize = vi.fn();
  const callback = vi.fn();
  const publish = createProjectionPublication(metadata.promise, authorize, vi.fn(), request.signal);
  const result = publish(callback);
  request.abort();
  await expect(result).rejects.toMatchObject({ name: 'AbortError' });
  await expect(publish(callback)).rejects.toMatchObject({ name: 'AbortError' });
  metadata.resolve('old-agent');
  await Promise.resolve();
  expect(authorize).not.toHaveBeenCalled();
  expect(callback).not.toHaveBeenCalled();
  publish.dispose();
});

it('checks the local epoch again between publication callbacks', async () => {
  let invalidated = false;
  const guard = () => {
    if (invalidated) throw new Error('Epoch changed');
  };
  const publish = createProjectionPublication(
    Promise.resolve('agent'),
    async () => undefined,
    guard,
    new AbortController().signal,
  );
  const first = publish(() => {
    invalidated = true;
  });
  const second = publish(vi.fn());
  await first;
  await expect(second).rejects.toThrow('Epoch changed');
  await expect(publish(vi.fn())).rejects.toThrow('Epoch changed');
  publish.dispose();
});

it.each(['metadata', 'authorization', 'callback'] as const)(
  'propagates %s failures without publishing later data',
  async (phase) => {
    const error = new Error('Failed read');
    const publish = createProjectionPublication(
      phase === 'metadata' ? Promise.reject(error) : Promise.resolve('agent'),
      async () => {
        if (phase === 'authorization') throw error;
      },
      vi.fn(),
      new AbortController().signal,
    );
    const later = vi.fn();
    const pending = [
      publish(() => {
        if (phase === 'callback') throw error;
      }),
      publish(later),
    ];
    const results = await Promise.allSettled(pending);
    expect(results.every((result) => result.status === 'rejected' && result.reason === error)).toBe(
      true,
    );
    expect(later).not.toHaveBeenCalled();
    await expect(publish(later)).rejects.toBe(error);
    publish.dispose();
  },
);

it.each(['dispose', 'abort'] as const)(
  'settles active waiters immediately on %s while authorization is unresolved',
  async (close) => {
    const proof = deferred<void>();
    const request = new AbortController();
    const publish = createProjectionPublication(
      Promise.resolve('agent'),
      () => proof.promise,
      vi.fn(),
      request.signal,
    );
    const callback = vi.fn();
    const result = publish(callback);
    await Promise.resolve();
    await Promise.resolve();
    if (close === 'dispose') publish.dispose();
    else request.abort();
    await expect(result).rejects.toMatchObject({ name: 'AbortError' });
    // A later rejection is still observed by flush and cannot publish anything.
    proof.reject(new Error('Late authorization failure'));
    await Promise.resolve();
    expect(callback).not.toHaveBeenCalled();
    publish.dispose();
  },
);

it('puts a reentrant publication behind another fresh check', async () => {
  const authorize = vi.fn(async () => undefined);
  const publish = createProjectionPublication(
    Promise.resolve('agent'),
    authorize,
    vi.fn(),
    new AbortController().signal,
  );
  const callback = vi.fn();
  let second!: Promise<void>;
  await publish(() => {
    second = publish(callback);
  });
  await second;
  expect(authorize).toHaveBeenCalledTimes(2);
  expect(callback).toHaveBeenCalledOnce();
  publish.dispose();
});

it.each(['dispose', 'abort'] as const)(
  'prevents later callbacks after synchronous %s',
  async (close) => {
    const request = new AbortController();
    const publish = createProjectionPublication(
      Promise.resolve('agent'),
      async () => undefined,
      vi.fn(),
      request.signal,
    );
    const callback = vi.fn();
    const first = publish(() => {
      if (close === 'dispose') publish.dispose();
      else request.abort();
    });
    const second = publish(callback);
    const results = await Promise.allSettled([first, second]);
    expect(results.every((result) => result.status === 'rejected')).toBe(true);
    expect(callback).not.toHaveBeenCalled();
    publish.dispose();
  },
);
