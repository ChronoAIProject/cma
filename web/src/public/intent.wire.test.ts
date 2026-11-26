import { afterEach, expect, it, vi } from 'vitest';
import { createIntent, readIntent, writeIntent } from './intent';
import { admit } from './admission';
import { publicClient } from '../test/public';

const key = 'cma:public-intent';
afterEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

it.each([
  { placementId: undefined, referrer: '', choice: undefined },
  { placementId: 'placement-a', referrer: '', choice: undefined },
  { placementId: undefined, referrer: 'https://publisher.example.test/readme', choice: undefined },
  { placementId: undefined, referrer: '', choice: 'new' as const },
  {
    placementId: 'placement-a',
    referrer: 'https://publisher.example.test/readme',
    choice: 'resume' as const,
  },
])(
  'retains exact dispatched JSON through storage recovery: %j',
  async ({ placementId, referrer, choice }) => {
    vi.spyOn(document, 'referrer', 'get').mockReturnValue(referrer);
    const original = createIntent(
      { kind: 'public_trigger', publicId: 'wire-example', placementId },
      choice,
    );
    const wireBody = JSON.stringify(original.body);
    const client = publicClient();
    vi.mocked(client.requests.createOrResumeBound).mockRejectedValueOnce(
      new Error('Unknown result'),
    );
    await expect(admit(client, original, 'owner-a')).rejects.toThrow('Unknown result');
    const restored = readIntent(original.path)!;
    expect(JSON.stringify(restored.body)).toBe(wireBody);
    await admit(client, restored, 'owner-a');
    const calls = vi.mocked(client.requests.createOrResumeBound).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[1]?.[2].idempotencyKey).toBe(calls[0]?.[2].idempotencyKey);
    expect(JSON.stringify(calls[1]?.[1])).toBe(JSON.stringify(calls[0]?.[1]));
  },
);

it('stops an unexpired legacy submitted packet instead of guessing its dispatched bytes', async () => {
  const legacy = {
    ...createIntent({
      kind: 'public_trigger',
      publicId: 'legacy-wire',
      placementId: 'placement-a',
    }),
    owner: 'owner-a',
    submitted: true,
  };
  writeIntent(legacy);
  const bytes = sessionStorage.getItem(key);
  const client = publicClient();
  await expect(admit(client, readIntent(legacy.path)!, 'owner-a')).rejects.toThrow(
    'original request is saved',
  );
  expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(key)).toBe(bytes);
});

it('does not attach a reordered body to an in-memory result or overwrite the original packet', async () => {
  const original = createIntent({
    kind: 'public_trigger',
    publicId: 'retained-wire',
    placementId: 'placement-a',
  });
  const client = publicClient();
  await admit(client, original, 'owner-a');
  const restored = readIntent(original.path)!;
  const bytes = sessionStorage.getItem(key);
  const reordered = {
    ...restored,
    body: { start_if_ready: true as const, placement_id: 'placement-a' },
  };
  expect(() => writeIntent(reordered)).toThrow();
  await expect(admit(client, reordered, 'owner-a')).rejects.toThrow();
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(key)).toBe(bytes);
});

it.each(['changed bytes', 'unknown body field'] as const)(
  'retains an invalid wire snapshot without admitting it: %s',
  (change) => {
    const original = createIntent({
      kind: 'public_trigger',
      publicId: 'invalid-wire',
      placementId: 'placement-a',
    });
    const record = {
      ...original,
      owner: 'owner-a',
      submitted: true,
      wireBody: JSON.stringify(original.body),
      body:
        change === 'changed bytes'
          ? { start_if_ready: true, placement_id: 'placement-a' }
          : { ...original.body, extra_command: true },
    };
    const bytes = JSON.stringify(record);
    sessionStorage.setItem(key, bytes);
    expect(readIntent(original.path)).toBeUndefined();
    expect(() => createIntent({ kind: 'public_trigger', publicId: 'invalid-wire' })).toThrow();
    expect(sessionStorage.getItem(key)).toBe(bytes);
  },
);

it('does not silently add proof to an already-submitted legacy command', () => {
  const legacy = {
    ...createIntent({ kind: 'public_trigger', publicId: 'legacy-wire' }),
    owner: 'owner-a',
    submitted: true,
  };
  writeIntent(legacy);
  const bytes = sessionStorage.getItem(key);
  expect(() => writeIntent({ ...legacy, wireBody: JSON.stringify(legacy.body) })).toThrow();
  expect(sessionStorage.getItem(key)).toBe(bytes);
});

it('permits a fresh explicit command after a known legacy credential refusal', async () => {
  const legacy = {
    ...createIntent({
      kind: 'public_trigger',
      publicId: 'legacy-refused',
      placementId: 'placement-a',
    }),
    owner: 'owner-a',
    submitted: true,
    credentialRecovery: 'refused' as const,
  };
  writeIntent(legacy);
  const retry = {
    ...legacy,
    key: crypto.randomUUID(),
    submitted: false,
    credentialRecovery: 'retry' as const,
  };
  writeIntent(retry);
  const client = publicClient();
  await admit(client, retry, 'owner-a');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledWith(legacy.publicId, legacy.body, {
    idempotencyKey: retry.key,
    expectedSubject: 'owner-a',
  });
  expect(readIntent(legacy.path)).toMatchObject({
    key: retry.key,
    submitted: true,
    wireBody: JSON.stringify(legacy.body),
  });
});

it('keeps the review fence when an older copy of the same command is saved', async () => {
  const client = publicClient();
  const original = createIntent({ kind: 'public_trigger', publicId: 'review-fence' });
  await admit(client, original, 'owner-a');
  const saved = readIntent(original.path)!;
  writeIntent({ ...saved, recoveryRequired: true });
  writeIntent(saved);
  const fenced = readIntent(original.path)!;
  expect(fenced.recoveryRequired).toBe(true);
  await expect(admit(client, fenced, 'owner-a')).rejects.toThrow('original request is saved');
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
});
