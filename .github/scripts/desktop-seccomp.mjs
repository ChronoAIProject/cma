// CI uses Docker; production uses an isolated Kata VM with working nested
// user namespaces. Keep Docker's pinned default restrictions, adding only
// Chromium's user/PID/network namespace creation and in-namespace chroot.
//
// The base profile is vendored so desktop evidence has no network dependency;
// a transient upstream 503 must not fail the check. seccomp-vendor/moby-default.json
// is an unmodified copy of
// https://raw.githubusercontent.com/moby/profiles/4b8214fc4971e06172938a1f1554ccff6647ee80/seccomp/default.json
// (moby/profiles commit 4b8214fc4971e06172938a1f1554ccff6647ee80).
// Copyright The Moby Authors; licensed under the Apache License, Version 2.0
// (SPDX: Apache-2.0). A copy of that license, from the same commit, is
// seccomp-vendor/MOBY-PROFILES-LICENSE; the upstream commit has no NOTICE file.
// The vendored file is unchanged and must keep its upstream sha256. The rules
// added below change only the derived profile this module writes.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';

const VENDORED_PROFILE = new URL('./seccomp-vendor/moby-default.json', import.meta.url);

export async function desktopSeccomp(destination) {
  const bytes = readFileSync(VENDORED_PROFILE);
  assert.equal(createHash('sha256').update(bytes).digest('hex'),
    '6416b47770785a41ac59073cdc77d9fe98517df2799dc83ef207e622de3053f6',
    'vendored moby seccomp profile differs from upstream commit 4b8214fc');
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
