import { constants, closeSync, fstatSync, openSync, readSync, realpathSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { limit, qualifySmoke, rawStatus, unavailable } from './smoke-evidence/contract.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
function readPrivate(path) {
  if (!isAbsolute(path) || realpathSync(path) !== path) throw Error();
  const inside = relative(root, path);
  if (!inside.startsWith('..' + '/') && !isAbsolute(inside)) throw Error();
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.uid !== process.getuid() || (stat.mode & 0o077) !== 0 || stat.size > limit) throw Error();
    const bytes = Buffer.alloc(limit + 1);
    let size = 0, count;
    do {
      count = readSync(fd, bytes, size, bytes.length - size, null);
      size += count;
    } while (count && size <= limit);
    if (size > limit) throw Error();
    return bytes.subarray(0, size);
  } finally { closeSync(fd); }
}

export function main(args) {
  if (args.length === 0) return { code: 0, result: unavailable() };
  if (args.length !== 8 || args[0] !== '--strict') return { code: 1, result: unavailable('invalid_invocation') };
  let resultsBytes;
  try {
    resultsBytes = readPrivate(args[7]);
    const [policyBytes, manifestBytes, beforeBytes, executionBytes, afterBytes] =
      [args[1], ...args.slice(3, 7)].map(readPrivate);
    const result = qualifySmoke({ policyBytes, policyHash: args[2], manifestBytes, beforeBytes, executionBytes, afterBytes, resultsBytes });
    return { code: result.qualification === 'verified' ? 0 : 1, result };
  } catch { return { code: 1, result: unavailable('file_unavailable', rawStatus(resultsBytes)) }; }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  if (process.argv.length === 3 && process.argv[2] === '--help') {
    process.stdout.write('Usage: node smoke-evidence.mjs [--strict POLICY POLICY_SHA256 MANIFEST BEFORE EXECUTION AFTER RESULTS]\n' +
      'Default: report binding_disabled without reading evidence. Strict: private absolute file paths outside the checkout; emit separate qualification JSON.\n' +
      'POLICY_SHA256 must come from protected release authority, not the evidence bundle. POLICY pins the candidate/run, observer/config hashes and distinct Ed25519 observer/harness public keys.\n' +
      'Signed observation/execution envelopes contain payload (exact JSON string) and signature (base64). No collector or signing keys are loaded here.\n' +
      'A verified result covers declared smoke components at two observation times, not continuous stability, mutations or full release qualification.\n');
  } else {
    const { code, result } = main(process.argv.slice(2));
    process.stdout.write(JSON.stringify(result) + '\n');
    process.exitCode = code;
  }
}
