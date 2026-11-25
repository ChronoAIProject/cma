import { afterEach, expect, it, vi } from 'vitest';
import type { Launch } from '@cma/client';
import { PUBLIC_RECORD, publicClient } from '../test/public';
import { admit } from './admission';
import { createIntent, forgetIntent, readIntent, writeIntent, type BrowserIntent } from './intent';

const route = { kind: 'public_trigger', publicId: 'recovery-example' } as const;
const path = '/t/recovery-example';
const owner = 'original-owner';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it.each([0, 1, -30 * 60 * 1000])(
  'retains submitted identity across clock offset %i without extending consent',
  async (offset) => {
    const client = publicClient();
    const original = createIntent(route);
    await admit(client, original, owner);
    const submitted = readIntent(path)!;
    vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + offset);
    expect(readIntent(path)).toEqual(submitted);
    await expect(admit(client, submitted, owner)).resolves.toEqual(PUBLIC_RECORD);
    expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
    expect(() => writeIntent(original)).toThrow('earlier request');
    expect(readIntent(path)).toEqual(submitted);
  },
);

it.each(['owner', 'id', 'publicId', 'path', 'body'] as const)(
  'refuses a different %s before attaching to a retained result',
  async (field) => {
    const client = publicClient();
    const original = createIntent(route);
    await admit(client, original, owner);
    const submitted = readIntent(path)!;
    const altered: BrowserIntent =
      field === 'body'
        ? { ...submitted, body: { ...submitted.body, choice: 'new' } }
        : field === 'owner'
          ? submitted
          : { ...submitted, [field]: 'different' };
    await expect(
      admit(client, altered, field === 'owner' ? 'other-owner' : owner),
    ).rejects.toThrow();
    expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
    expect(readIntent(path)).toEqual(submitted);
  },
);

it('does not dispatch an expired packet after reload loses its in-memory result', async () => {
  const client = publicClient();
  const original = { ...createIntent(route), owner, submitted: true };
  writeIntent(original);
  const retained = sessionStorage.getItem('cma:public-intent');
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt);
  await expect(admit(client, original, owner)).rejects.toThrow('Your original request is saved');
  expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  expect(sessionStorage.getItem('cma:public-intent')).toBe(retained);
});

it('keeps an expired packet when capacity eviction removes its unresolved result', async () => {
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockReturnValueOnce(new Promise<Launch>(() => {}));
  const original = createIntent(route);
  void admit(client, original, owner);
  const submitted = readIntent(path)!;
  // Isolate result-cache eviction from the independently tested storage barrier.
  forgetIntent(original.id);
  for (let index = 0; index < 50; index += 1) {
    const next = createIntent(route);
    await admit(client, next, owner);
    forgetIntent(next.id);
  }
  writeIntent(submitted);
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + 1);
  await expect(admit(client, submitted, owner)).rejects.toThrow('Your original request is saved');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledTimes(51);
  expect(readIntent(path)).toEqual(submitted);
});

it('preserves an unknown result past expiry without resending its command', async () => {
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockRejectedValueOnce(
    new Error('connection lost'),
  );
  const original = createIntent(route);
  await expect(admit(client, original, owner)).rejects.toThrow('connection lost');
  const submitted = readIntent(path)!;
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + 1);
  await expect(admit(client, submitted, owner)).rejects.toThrow('Your original request is saved');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(readIntent(path)).toEqual(submitted);
});
