import { afterEach, expect, it, vi } from 'vitest';
import { admit } from './admission';
import { createIntent, forgetIntent, readIntent, writeIntent } from './intent';
import { publicClient } from '../test/public';

const route = { kind: 'public_trigger', publicId: 'entry-a', placementId: 'placement-a' } as const;
const path = '/t/entry-a/placement-a';
const storageKey = 'cma:public-intent';

afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

async function unknownRequest() {
  const client = publicClient();
  vi.mocked(client.requests.createOrResumeBound).mockRejectedValueOnce(new Error('Unknown result'));
  const original = createIntent(route, 'new');
  await expect(admit(client, original, 'owner-a')).rejects.toThrow('Unknown result');
  return { client, original: readIntent(path)!, bytes: sessionStorage.getItem(storageKey) };
}

it.each([{ ...route, publicId: 'entry-b' }, { ...route, placementId: 'placement-b' }, route])(
  'stops another command before replacing unresolved A: %j',
  async (nextRoute) => {
    const { client, original, bytes } = await unknownRequest();
    expect(() => createIntent(nextRoute)).toThrow('earlier request');
    expect(sessionStorage.getItem(storageKey)).toBe(bytes);
    await admit(client, original, 'owner-a');
    const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1]).toEqual(calls[0]);
    forgetIntent(original.id);
    expect(createIntent(nextRoute).id).not.toBe(original.id);
  },
);

it('keeps the occupied slot after expiry, without resending or re-keying A', async () => {
  const { client, original, bytes } = await unknownRequest();
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt + 1);
  expect(() => createIntent({ ...route, publicId: 'entry-b' })).toThrow('earlier request');
  await expect(admit(client, original, 'owner-a')).rejects.toThrow('original request is saved');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(storageKey)).toBe(bytes);
});

it.each(['owner', 'id', 'key', 'path', 'publicId', 'expiresAt', 'body'] as const)(
  'does not overwrite an unresolved command with altered %s',
  async (field) => {
    const { original, bytes } = await unknownRequest();
    const altered =
      field === 'body'
        ? { ...original, body: { ...original.body, choice: 'resume' as const } }
        : field === 'expiresAt'
          ? { ...original, expiresAt: original.expiresAt + 1 }
          : { ...original, [field]: 'different' };
    expect(() => writeIntent(altered)).toThrow('earlier request');
    expect(sessionStorage.getItem(storageKey)).toBe(bytes);
    forgetIntent('another-intent');
    expect(sessionStorage.getItem(storageKey)).toBe(bytes);
  },
);

it('keeps submitted state when the same command is saved from an older observer copy', async () => {
  const { original } = await unknownRequest();
  writeIntent({ ...original, submitted: false });
  expect(readIntent(path)).toEqual(original);
});

it('allows credential re-keying only after a known refusal of the original command', async () => {
  const { original } = await unknownRequest();
  const refused = { ...original, credentialRecovery: 'refused' as const };
  writeIntent(refused);
  const retry = { ...refused, key: crypto.randomUUID(), credentialRecovery: 'retry' as const };
  writeIntent(retry);
  expect(readIntent(path)).toEqual(retry);
});

it('can replace an expired unsubmitted intent without extending its consent', () => {
  const original = createIntent(route);
  vi.spyOn(Date, 'now').mockReturnValue(original.expiresAt);
  expect(readIntent(path)).toBeUndefined();
  expect(createIntent({ ...route, publicId: 'entry-b' }).publicId).toBe('entry-b');
});

it.each([
  { owner: undefined },
  { path: 'https://example.test/unsafe' },
  { body: { start_if_ready: true, placement_id: 'different' } },
])('preserves an unreadable legacy submitted packet: %j', async (override) => {
  const { original } = await unknownRequest();
  const bytes = JSON.stringify({ ...original, ...override });
  sessionStorage.setItem(storageKey, bytes);
  expect(readIntent(path)).toBeUndefined();
  expect(() => createIntent(route)).toThrow();
  expect(sessionStorage.getItem(storageKey)).toBe(bytes);
});

it('does not dispatch when reading the occupied slot fails', async () => {
  const { client, original, bytes } = await unknownRequest();
  const read = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  expect(() => admit(client, original, 'owner-a')).toThrow('Storage unavailable');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  read.mockRestore();
  expect(sessionStorage.getItem(storageKey)).toBe(bytes);
});

it('stops before replacing corrupt JSON and provides no unvalidated recovery link', () => {
  sessionStorage.setItem(storageKey, '{corrupt');
  expect(() => createIntent(route)).toThrow('could not be read safely');
  expect(sessionStorage.getItem(storageKey)).toBe('{corrupt');
});
