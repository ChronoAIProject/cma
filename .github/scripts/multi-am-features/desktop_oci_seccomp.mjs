import assert from 'node:assert/strict';
import {readFileSync, writeFileSync} from 'node:fs';
import {desktopSeccomp} from '../desktop-seccomp.mjs';

export async function desktopOciSeccomp(destination, kernel) {
  assert.match(kernel, /^\d+\.\d+/);
  const [major, minor] = kernel.split('.').map(part => Number.parseInt(part, 10));
  await desktopSeccomp(destination);
  const original = JSON.parse(readFileSync(destination, 'utf8'));
  const amd64 = original.archMap.find(row => row.architecture === 'SCMP_ARCH_X86_64');
  const enabled = (test, including) => Object.entries(test ?? {}).every(([kind, value]) => {
    switch (kind) {
      case 'arches':
        return including === value.includes('amd64');
      case 'caps':
        return !including;
      case 'minKernel': {
        const [minimumMajor, minimumMinor] = value.split('.').map(Number);
        const meetsMinimum = major > minimumMajor || major === minimumMajor && minor >= minimumMinor;
        return including === meetsMinimum;
      }
      default:
        throw Error('unsupported_seccomp_filter');
    }
  });
  const syscalls = original.syscalls
    .filter(row => enabled(row.includes, true) && enabled(row.excludes, false))
    .map(({names, action, args, errnoRet}) => ({names, action, args, errnoRet}));
  assert(syscalls.length > 0, 'empty desktop syscall policy');
  const profile = {architectures: [amd64.architecture, ...amd64.subArchitectures],
    defaultAction: original.defaultAction, defaultErrnoRet: original.defaultErrnoRet, syscalls};
  writeFileSync(destination, JSON.stringify(profile));
}

await desktopOciSeccomp(process.argv[2], process.argv[3]);
