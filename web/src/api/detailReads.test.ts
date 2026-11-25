import { expect, it, vi } from 'vitest';
import { agentClient, AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import type { CmaWebClient } from './client';
import { coalesceDetailReads, detailReadSource } from './detailReads';

it('identifies a wrapped source without modifying clients or mixing account identities', () => {
  const original = Object.freeze(agentClient());
  const wrapped = coalesceDetailReads(original);
  const nested = coalesceDetailReads(wrapped.client);
  expect(detailReadSource(original)).toBe(original);
  expect(detailReadSource(wrapped.client)).toBe(original);
  expect(detailReadSource(nested.client)).toBe(original);
  const other = agentClient();
  expect(detailReadSource(coalesceDetailReads(other).client)).toBe(other);
  expect(detailReadSource(other)).not.toBe(original);
  wrapped.dispose();
  nested.dispose();
  expect(detailReadSource(wrapped.client)).toBe(original);
});

it('preserves retained history limits and cache policy without joining another reader', async () => {
  const readers: (() => void)[] = [];
  const read = vi.fn<CmaWebClient['readAgentTranscript']>(
    () =>
      new Promise((resolve) => {
        readers.push(() => resolve(AGENT_TRANSCRIPT));
      }),
  );
  const scope = coalesceDetailReads(agentClient({ readAgentTranscript: read }));
  const ordinary = scope.client.readAgentTranscript('agt_1');
  const bounded = scope.client.readAgentTranscript('agt_1', {
    cache: 'no-store',
    maxPages: 256,
    maxBytes: 16 * 1024 * 1024,
  });
  const exportRead = scope.client
    .readAgentTranscript('agt_1', {
      cache: 'no-store',
      maxPages: 512,
      maxBytes: 32 * 1024 * 1024,
    })
    .catch((error: unknown) => error);
  await Promise.resolve();
  expect(read).toHaveBeenCalledTimes(3);
  expect(read.mock.calls[1]?.[1]).toEqual({
    signal: expect.any(AbortSignal),
    cache: 'no-store',
    maxPages: 256,
    maxBytes: 16 * 1024 * 1024,
  });
  expect(read.mock.calls[2]?.[1]).toEqual({
    signal: expect.any(AbortSignal),
    cache: 'no-store',
    maxPages: 512,
    maxBytes: 32 * 1024 * 1024,
  });
  readers[0]!();
  readers[1]!();
  await expect(ordinary).resolves.toBe(AGENT_TRANSCRIPT);
  await expect(bounded).resolves.toBe(AGENT_TRANSCRIPT);
  scope.dispose();
  expect(read.mock.calls[2]?.[1]?.signal?.aborted).toBe(true);
  expect(await exportRead).toMatchObject({ name: 'AbortError' });
});

function setup() {
  let resolve!: (value: typeof AGENT_DETAIL) => void;
  let reject!: (error: unknown) => void;
  const readAgent = vi.fn(
    () =>
      new Promise<typeof AGENT_DETAIL>((yes, no) => {
        resolve = yes;
        reject = no;
      }),
  );
  const original = agentClient({ readAgent });
  const scoped = coalesceDetailReads(original);
  return {
    ...scoped,
    original,
    readAgent,
    resolve: () => resolve(AGENT_DETAIL),
    reject: () => reject(new Error('unavailable')),
  };
}

it('shares concurrent identical detail reads but never a settled value', async () => {
  const h = setup();
  const first = h.client.readAgent('agt_1');
  const second = h.client.readAgent('agt_1');
  await Promise.resolve();
  expect(h.readAgent).toHaveBeenCalledTimes(1);
  h.resolve();
  expect(await first).toEqual(AGENT_DETAIL);
  expect(await second).toEqual(AGENT_DETAIL);
  const next = h.client.readAgent('agt_1');
  await Promise.resolve();
  expect(h.readAgent).toHaveBeenCalledTimes(2);
  h.resolve();
  await next;
});

it('isolates subscriber cancellation and aborts the transport only when no reader remains', async () => {
  const h = setup();
  const a = new AbortController();
  const b = new AbortController();
  const first = h.client.readAgent('agt_1', { signal: a.signal }).catch((error: unknown) => error);
  const second = h.client.readAgent('agt_1', { signal: b.signal }).catch((error: unknown) => error);
  await Promise.resolve();
  const transport = vi.mocked(h.readAgent).mock.calls[0] as unknown as [
    string,
    { signal: AbortSignal },
  ];
  a.abort();
  expect(await first).toBe(a.signal.reason);
  expect(transport[1].signal.aborted).toBe(false);
  b.abort();
  expect(await second).toBe(b.signal.reason);
  expect(transport[1].signal.aborted).toBe(true);
  const next = h.client.readAgent('agt_1');
  await Promise.resolve();
  expect(h.readAgent).toHaveBeenCalledTimes(2);
  h.resolve();
  await next;
});

it('clears failures, separates identities and IDs, and fences disposed scopes', async () => {
  const h = setup();
  const first = h.client.readAgent('agt_1').catch((error: unknown) => error);
  await Promise.resolve();
  h.reject();
  expect(await first).toBeInstanceOf(Error);
  const second = h.client.readAgent('agt_1').catch((error: unknown) => error);
  const different = h.client.readAgent('agt_2').catch((error: unknown) => error);
  const other = coalesceDetailReads(h.original);
  const separate = other.client.readAgent('agt_1').catch((error: unknown) => error);
  await Promise.resolve();
  expect(h.readAgent).toHaveBeenCalledTimes(4);
  h.dispose();
  expect(await second).toMatchObject({ name: 'AbortError' });
  expect(await different).toMatchObject({ name: 'AbortError' });
  other.dispose();
  expect(await separate).toMatchObject({ name: 'AbortError' });
});

it.each([
  'readAgentTranscript',
  'readAgentUsage',
  'readAgentResources',
  'readAgentSource',
  'readAgentProvenance',
] as const)(
  'shares pending %s across sibling readers without reusing completed authority',
  async (method) => {
    let finish!: () => void;
    const read = vi.fn(
      () =>
        new Promise<never>((resolve) => {
          finish = () => resolve({} as never);
        }),
    );
    const source = agentClient({ [method]: read });
    const scope = coalesceDetailReads(source);
    const first = scope.client[method]('agt_1');
    const second = scope.client[method]('agt_1');
    await Promise.resolve();
    expect(read).toHaveBeenCalledOnce();
    finish();
    await Promise.all([first, second]);
    const fresh = scope.client[method]('agt_1');
    await Promise.resolve();
    expect(read).toHaveBeenCalledTimes(2);
    finish();
    await fresh;
    scope.dispose();
  },
);
