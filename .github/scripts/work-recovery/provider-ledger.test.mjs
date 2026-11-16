import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { openLedger } from './provider-ledger.mjs';

function directory(t) {
  const root = mkdtempSync(join(tmpdir(), 'ordinary-work-ledger-'));
  t.after(() => rmSync(root, { recursive: true }));
  return root;
}

test('exclusive ownership refuses a competing ledger without deleting the lock', t => {
  const root = directory(t);
  const ledger = openLedger(root);
  try {
    assert.throws(() => openLedger(root), /EEXIST/);
    assert(existsSync(join(root, 'provider.lock')));
    ledger.append('mint_received', { request: 1 });
    assert.equal(ledger.snapshot().received_mint_posts, 1);
    assert.equal(ledger.snapshot().attempted_sends, null);
  } finally { ledger.close(); }
  assert(!existsSync(join(root, 'provider.lock')));
});

test('partial or corrupt ledger cannot silently reset effect counts', t => {
  const root = directory(t);
  const ledger = openLedger(root);
  ledger.append('mint_received', { request: 1 });
  ledger.close();
  const path = join(root, 'provider.jsonl');
  const original = readFileSync(path, 'utf8');
  for (const content of [original.trimEnd(), original.replace('mint_received', 'mint_refused'), '{bad}\n']) {
    writeFileSync(path, content);
    assert.throws(() => openLedger(root));
    assert(!existsSync(join(root, 'provider.lock')));
    assert.equal(readFileSync(path, 'utf8'), content);
  }
});

test('state paths require private regular files and reject symlinks', t => {
  const root = directory(t);
  chmodSync(root, 0o755);
  assert.throws(() => openLedger(root), /private_fixture_directory/);
  chmodSync(root, 0o700);
  const other = join(root, 'other');
  writeFileSync(other, 'preserve', { mode: 0o600 });
  symlinkSync(other, join(root, 'provider.jsonl'));
  assert.throws(() => openLedger(root), /ELOOP/);
  assert.equal(readFileSync(other, 'utf8'), 'preserve');
  assert(!existsSync(join(root, 'provider.lock')));
});

test('event allowlist refuses secret or non-fixture metadata before persistence', t => {
  const root = directory(t);
  const ledger = openLedger(root);
  const secret = randomBytes(32).toString('hex');
  try {
    assert.throws(() => ledger.append('mint_accepted', { full_key: secret }), /invalid_fixture_event/);
    assert.throws(() => ledger.append('mint_accepted', { name: 'real-provider-name' }), /invalid_fixture_event/);
    assert.throws(() => ledger.append('mint_accepted', { request: 1 }), /invalid_fixture_event/);
    assert.throws(() => ledger.append('invented'), /fixture_ledger_closed_or_full/);
    assert.equal(readFileSync(join(root, 'provider.jsonl'), 'utf8'), '');
    assert.equal(ledger.snapshot().events.length, 0);
  } finally { ledger.close(); }
  assert.throws(() => ledger.append('mint_received', { request: 1 }), /fixture_ledger_closed_or_full/);
});
