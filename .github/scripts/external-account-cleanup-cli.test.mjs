import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, readFileSync } from 'node:fs';
import { canaries, line, harness } from './staging-authority-cli.fixture.mjs';

for (const mode of ['confirmed', 'missing', 'pending', 'untrusted', 'tampered', 'legacy', 'cma-incomplete', 'wrong-fixture']) {
  test(`CLI external cleanup ${mode} preserves private diagnostics and pending history`, (t) => {
    const h = harness(t);
    const f = h.cleanup;
    if (mode === 'untrusted') {
      delete h.env.CMA_E2E_CLEANUP_TRUST_FILE;
      delete h.env.CMA_E2E_CLEANUP_TRUST_SHA256;
    }
    if (mode === 'tampered') f.confirmed.receipt.payload.receiptId = canaries[3];
    if (mode === 'legacy') delete f.pending.schema;
    if (mode === 'wrong-fixture') h.env.CMA_E2E_FIXTURE_OWNER = 'other-fixture-owner';
    let journal = h.journal;
    if (mode === 'cma-incomplete') journal = journal.slice(1);
    if (mode === 'pending') journal = journal.filter((row) => row !== f.confirmed);
    if (mode === 'missing') journal = journal.filter((row) => row !== f.pending && row !== f.confirmed);
    const bytes = journal.map((row) => JSON.stringify(row)).join('\n');
    h.write('cleanup.ndjson', bytes);
    const result = h.run('audit');
    assert.equal(readFileSync(h.path('cleanup.ndjson'), 'utf8'), bytes);
    // The synthetic campaign still lacks the separate live connection outcome matrix.
    assert.equal(result.status, 1);
    assert.equal(result.stderr, line('audit', mode === 'confirmed' ? 'connection_coverage' : 'cleanup_journal'));
    assert.equal(readFileSync(h.path('output'), 'utf8'), '');
    assert.equal(existsSync(h.path('attestation.json')), false);
  });
}
