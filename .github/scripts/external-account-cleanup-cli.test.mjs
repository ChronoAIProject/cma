import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, readFileSync } from 'node:fs';
import { externalCleanupFixture } from './external-account-cleanup.fixture.mjs';
import { hash, canaries, line, harness } from './staging-authority-cli.fixture.mjs';

for (const mode of ['confirmed', 'pending', 'untrusted', 'tampered', 'legacy', 'cma-incomplete']) {
  test(`CLI external cleanup ${mode} preserves private diagnostics and pending history`, (t) => {
    const h = harness(t);
    const f = externalCleanupFixture();
    const trust = JSON.stringify(f.authority.trust);
    h.write('cleanup-trust.json', trust);
    h.env.CMA_E2E_CLEANUP_TRUST_FILE = h.path('cleanup-trust.json');
    h.env.CMA_E2E_CLEANUP_TRUST_SHA256 = hash(trust);
    if (mode === 'untrusted') {
      delete h.env.CMA_E2E_CLEANUP_TRUST_FILE;
      delete h.env.CMA_E2E_CLEANUP_TRUST_SHA256;
    }
    if (mode === 'tampered') f.confirmed.receipt.payload.receiptId = canaries[3];
    if (mode === 'legacy') delete f.pending.schema;
    const journal = [...(mode === 'cma-incomplete' ? h.journal.slice(1) : h.journal), f.pending, ...(mode === 'pending' ? [] : [f.confirmed])];
    const bytes = journal.map((row) => JSON.stringify(row)).join('\n');
    h.write('cleanup.ndjson', bytes);
    const result = h.run('audit');
    assert.equal(readFileSync(h.path('cleanup.ndjson'), 'utf8'), bytes);
    if (mode === 'confirmed') {
      assert.equal(result.status, 0);
      assert.equal(readFileSync(h.path('output'), 'utf8'), 'publishable=true\n');
    } else {
      assert.equal(result.status, 1);
      assert.equal(result.stderr, line('audit', 'cleanup_journal'));
      assert.equal(readFileSync(h.path('output'), 'utf8'), '');
      assert.equal(existsSync(h.path('attestation.json')), false);
    }
  });
}
