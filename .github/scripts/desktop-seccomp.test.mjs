// The desktop seccomp profile is derived from vendored bytes, never the network.
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import test from 'node:test';
import { desktopSeccomp } from './desktop-seccomp.mjs';

test('derives the desktop profile offline from the pinned vendored base', async t => {
  const directory = mkdtempSync(resolve(tmpdir(), 'cma-desktop-seccomp-test-'));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  t.mock.method(globalThis, 'fetch', () => { throw new Error('network is unavailable'); });
  const destination = resolve(directory, 'seccomp.json');
  await desktopSeccomp(destination);
  assert.equal(globalThis.fetch.mock.callCount(), 0);
  const base = JSON.parse(readFileSync(new URL('./seccomp-vendor/moby-default.json', import.meta.url)));
  const profile = JSON.parse(readFileSync(destination));
  assert.equal(profile.defaultAction, 'SCMP_ACT_ERRNO');
  assert.deepEqual(profile.syscalls.slice(0, base.syscalls.length), base.syscalls);
  const added = profile.syscalls.slice(base.syscalls.length);
  assert.deepEqual(added.map(rule => rule.names[0]),
    ['clone', ...Array(7).fill('unshare'), 'chroot']);
  assert(added.every(rule => rule.action === 'SCMP_ACT_ALLOW'));
});
