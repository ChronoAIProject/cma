// Qualify the exact local image: no rebuild, registry publication or runtime activation.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { validateImage } from './runtime-image-evidence.mjs';

export function elfContract(bytes) {
  assert(bytes.length >= 64);
  assert.equal(bytes.subarray(0, 4).toString('hex'), '7f454c46');
  assert.equal(bytes[4], 2, 'ELF64 required');
  assert.equal(bytes[5], 1, 'little endian required');
  assert.equal(bytes.readUInt16LE(18), 62, 'amd64 required');
  const offset = Number(bytes.readBigUInt64LE(32));
  const size = bytes.readUInt16LE(54), count = bytes.readUInt16LE(56);
  assert(size >= 56 && count > 0 && count < 512);
  assert(Number.isSafeInteger(offset) && offset >= 64 && offset + size * count <= bytes.length);
  let interpreter = false;
  const needed = [];
  for (let index = 0; index < count; index++) {
    const header = offset + index * size;
    const type = bytes.readUInt32LE(header);
    interpreter ||= type === 3;
    if (type !== 2) continue;
    const start = Number(bytes.readBigUInt64LE(header + 8));
    const length = Number(bytes.readBigUInt64LE(header + 32));
    assert(Number.isSafeInteger(start) && Number.isSafeInteger(length) && start >= 0 && length >= 0 && start + length <= bytes.length && length % 16 === 0);
    for (let entry = start; entry < start + length; entry += 16) {
      const tag = bytes.readBigInt64LE(entry);
      if (tag === 0n) break;
      if (tag === 1n) needed.push(bytes.readBigUInt64LE(entry + 8).toString());
    }
  }
  assert(!interpreter, 'runtime must not require an ELF interpreter');
  assert.equal(needed.length, 0, 'runtime must not require dynamic libraries, including SQLite');
  return { elf: 'ELF64', architecture: 'amd64', interpreter: null, needed, sha256: createHash('sha256').update(bytes).digest('hex') };
}

export function validateSqlite(proof) {
  assert.deepEqual(Object.keys(proof).sort(), ['anchored_import_create_commit_reopen', 'compile_options', 'create_commit_reopen', 'indexed_retirement_copy_reopen_refusal', 'journal_mode', 'retirement_archive_copy_reopen_refusal', 'sqlite_source_id', 'sqlite_version', 'synchronous', 'synthetic_retirement_move_restore']);
  assert.equal(proof.sqlite_version, '3.54.1'); // bundled libsqlite3-sys 0.39.1
  assert.match(proof.sqlite_source_id, /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} [a-f0-9]{64}$/);
  assert(Array.isArray(proof.compile_options) && proof.compile_options.length > 0 && proof.compile_options.length <= 256);
  assert(proof.compile_options.every(option => typeof option === 'string' && option.length <= 256));
  assert(proof.compile_options.includes('THREADSAFE=1'));
  assert.equal(proof.journal_mode, 'delete');
  assert.equal(proof.synchronous, 'extra');
  assert.equal(proof.create_commit_reopen, 'passed');
  assert.equal(proof.anchored_import_create_commit_reopen, 'passed');
  assert.equal(proof.retirement_archive_copy_reopen_refusal, 'passed');
  assert.equal(proof.synthetic_retirement_move_restore, 'passed');
  assert.equal(proof.indexed_retirement_copy_reopen_refusal, 'passed');
}

export function qualify(imageRef, revision, destination) {
  const docker = args => execFileSync('docker', args, { encoding: 'utf8', timeout: 180000, maxBuffer: 1024 * 1024 });
  const image = JSON.parse(docker(['image', 'inspect', imageRef]))[0];
  validateImage(image, revision);
  const temporary = mkdtempSync(resolve(tmpdir(), 'cma-ledger-elf-'));
  const name = `cma-ledger-probe-${randomUUID()}`;
  const result = { schema: 'cma-runtime-ledger-qualification-v1', revision, image: image.Id, result: 'fail', cleanup: false };
  let created = false;
  try {
    docker(['create', '--name', name, '--label', `dev.chrono.cma.ledger-probe=${name}`, '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges=true', '--user', '10001:10001', '--pids-limit', '32', '--memory', '256m', '--cpus', '2', '--tmpfs', '/ledger:rw,nosuid,nodev,size=134217728,mode=700,uid=10001,gid=10001', '--entrypoint', '/usr/local/bin/layer4-runtime', image.Id, '--verify-ledger-storage', '/ledger/probe']);
    created = true;
    const executable = resolve(temporary, 'layer4-runtime');
    docker(['cp', `${name}:/usr/local/bin/layer4-runtime`, executable]);
    result.binary = elfContract(readFileSync(executable));
    result.sqlite = JSON.parse(docker(['start', '--attach', name]));
    assert.equal(JSON.parse(docker(['inspect', name]))[0].State.ExitCode, 0);
    validateSqlite(result.sqlite);
    result.result = 'pass';
  } finally {
    if (created) docker(['rm', '--force', name]);
    rmSync(temporary, { recursive: true, force: true });
    result.cleanup = true;
    writeFileSync(destination, JSON.stringify(result, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [image, revision, destination] = process.argv.slice(2);
  qualify(image, revision, destination);
}
