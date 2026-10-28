// CI uses Docker; production uses an isolated Kata VM with working nested
// user namespaces. Keep Docker's pinned default restrictions, adding only
// Chromium's user/PID/network namespace creation and in-namespace chroot.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { writeFileSync } from 'node:fs';

export async function desktopSeccomp(destination) {
  const source = 'https://raw.githubusercontent.com/moby/profiles/4b8214fc4971e06172938a1f1554ccff6647ee80/seccomp/default.json';
  const response = await fetch(source, { signal: AbortSignal.timeout(30000) });
  assert(response.ok, `seccomp profile download: ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  assert.equal(createHash('sha256').update(bytes).digest('hex'),
    '6416b47770785a41ac59073cdc77d9fe98517df2799dc83ef207e622de3053f6');
  const profile = JSON.parse(bytes);
  assert.equal(profile.defaultAction, 'SCMP_ACT_ERRNO');
  // All other namespace flags (mount, IPC, UTS, cgroup) remain denied. clone3
  // retains the default ENOSYS response so libc uses argument-filtered clone.
  const namespaces = 0x7e020000;
  const chromiumNamespaces = 0x10000000 | 0x20000000 | 0x40000000;
  profile.syscalls.push({
    names: ['clone'], action: 'SCMP_ACT_ALLOW',
    args: [{ index: 0, value: namespaces & ~chromiumNamespaces, valueTwo: 0, op: 'SCMP_CMP_MASKED_EQ' }],
    includes: { arches: ['amd64', 'arm64'] },
  });
  for (const flags of [0x10000000, 0x20000000, 0x40000000,
    0x30000000, 0x50000000, 0x60000000, 0x70000000]) {
    profile.syscalls.push({
      names: ['unshare'], action: 'SCMP_ACT_ALLOW',
      args: [{ index: 0, value: flags, op: 'SCMP_CMP_EQ' }],
    });
  }
  // With all outer capabilities dropped, chroot is usable only after entering
  // the nested user namespace. Chromium uses it to remove filesystem access.
  profile.syscalls.push({ names: ['chroot'], action: 'SCMP_ACT_ALLOW' });
  writeFileSync(destination, JSON.stringify(profile));
}
