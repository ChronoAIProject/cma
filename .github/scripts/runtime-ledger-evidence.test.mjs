import assert from 'node:assert/strict';
import test from 'node:test';
import { elfContract, validateSqlite } from './runtime-ledger-evidence.mjs';
function elf() {
  const bytes = Buffer.alloc(192);
  bytes.write('7f454c46', 0, 'hex'); bytes[4] = 2; bytes[5] = 1;
  bytes.writeUInt16LE(62, 18); bytes.writeBigUInt64LE(64n, 32);
  bytes.writeUInt16LE(56, 54); bytes.writeUInt16LE(1, 56);
  bytes.writeUInt32LE(1, 64);
  return bytes;
}
test('static ELF passes and dynamic/interpreter or malformed binaries refuse', () => {
  const bytes = elf();
  assert.deepEqual(elfContract(bytes).needed, []);
  for (const mutate of [b => b[5] = 2, b => b.writeUInt16LE(183, 18), b => b.writeUInt32LE(3, 64), b => b.writeBigUInt64LE(1000n, 32)]) {
    const changed = Buffer.from(bytes); mutate(changed); assert.throws(() => elfContract(changed));
  }
  const dynamic = elf();
  dynamic.writeUInt32LE(2, 64); dynamic.writeBigUInt64LE(128n, 72); dynamic.writeBigUInt64LE(32n, 96);
  dynamic.writeBigInt64LE(1n, 128); dynamic.writeBigUInt64LE(1n, 136);
  assert.throws(() => elfContract(dynamic));
  assert.throws(() => elfContract(Buffer.alloc(20)));
});
test('qualification requires pinned SQLite, actual options and EXTRA reopen proof', () => {
  const proof = { sqlite_version: '3.54.1', sqlite_source_id: '2026-10-01 00:00:00 ' + 'a'.repeat(64), compile_options: ['THREADSAFE=1'], journal_mode: 'delete', synchronous: 'extra', create_commit_reopen: 'passed', anchored_import_create_commit_reopen: 'passed', retirement_archive_copy_reopen_refusal: 'passed' };
  validateSqlite(proof);
  for (const change of [{sqlite_version:'3.0.0'}, {sqlite_source_id:'unknown'}, {compile_options:[]}, {synchronous:'full'}, {journal_mode:'wal'}, {create_commit_reopen:'failed'}, {anchored_import_create_commit_reopen:'failed'}, {anchored_import_create_commit_reopen:undefined}, {retirement_archive_copy_reopen_refusal:'failed'}, {retirement_archive_copy_reopen_refusal:undefined}, {extra:true}]) {
    assert.throws(() => validateSqlite({...proof, ...change}));
  }
});
