import { afterEach, expect, it, vi } from 'vitest';
import { createIntent, forgetIntent, readIntent, writeIntent } from './intent';
import { admit } from './admission';
import { publicClient } from '../test/public';

const route = {
  kind: 'public_trigger',
  publicId: 'public-example',
  placementId: 'plc_readme',
} as const;
const path = '/t/public-example/plc_readme';
afterEach(() => {
  sessionStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('expires after fifteen minutes, matches the exact route, and stores no prompt', () => {
  vi.useFakeTimers();
  const intent = createIntent(route);
  expect(readIntent(path)).toEqual(intent);
  expect(readIntent('/t/public-example/other')).toBeUndefined();
  expect(sessionStorage.getItem('cma:public-intent')).not.toContain('first_message');
  vi.advanceTimersByTime(15 * 60 * 1000);
  expect(readIntent(path)).toBeUndefined();
});

it.each([
  null,
  {},
  { expiresAt: 'forever' },
  { body: { start_if_ready: false } },
  { body: { start_if_ready: true, choice: 'unsafe' } },
  { body: { start_if_ready: true, placement_id: {} } },
  { body: { start_if_ready: true, referrer_origin: 'javascript:alert(1)' } },
  { body: { start_if_ready: true, referrer_origin: 'https://example.test/private' } },
])('rejects malformed browser state %j', (override) => {
  const intent = createIntent(route);
  sessionStorage.setItem(
    'cma:public-intent',
    JSON.stringify(override === null ? null : { ...intent, ...override }),
  );
  if (override !== null && Object.keys(override).length === 0)
    expect(readIntent(path)).toEqual(intent);
  else expect(readIntent(path)).toBeUndefined();
});

it('ignores invalid JSON and only forgets the matching intent', () => {
  const intent = createIntent(route);
  forgetIntent('different');
  expect(readIntent(path)).toEqual(intent);
  forgetIntent(intent.id);
  expect(readIntent(path)).toBeUndefined();
  sessionStorage.setItem('cma:public-intent', '{bad');
  expect(readIntent(path)).toBeUndefined();
  expect(() => forgetIntent(intent.id)).not.toThrow();
});

it('coalesces concurrent admission and rejects another owner before network I/O', async () => {
  const client = publicClient();
  const intent = createIntent(route);
  await Promise.all([admit(client, intent, 'owner'), admit(client, intent, 'owner')]);
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
  writeIntent({ ...intent, owner: 'owner', submitted: true });
  await expect(admit(client, readIntent(path)!, 'other')).rejects.toThrow();
  expect(client.requests.createOrResumeBound).toHaveBeenCalledOnce();
});

it.each(['refused', 'retry'] as const)(
  'retains owner-bound %s recovery without changing its packet',
  (credentialRecovery) => {
    const intent = { ...createIntent(route, 'new'), owner: 'original-owner', credentialRecovery };
    writeIntent(intent);
    expect(readIntent(path)).toEqual(intent);
  },
);

it.each([
  { credentialRecovery: 'refused' },
  { credentialRecovery: 'retry', owner: false },
  { credentialRecovery: 'unrecognized', owner: 'original-owner' },
])('rejects a recovery marker without its valid original owner %j', (recovery) => {
  const intent = createIntent(route);
  sessionStorage.setItem('cma:public-intent', JSON.stringify({ ...intent, ...recovery }));
  expect(readIntent(path)).toBeUndefined();
});
