import assert from 'node:assert/strict';
import test from 'node:test';
import { existsSync, readFileSync } from 'node:fs';
import { connectionCaseTitles, missingConnectionCoverage } from './connection-journey-evidence.mjs';
import { bindConnectionFixtures, requireConnectionCoverage, validateConnectionCases } from './connection-campaign.mjs';
import { campaignFixture } from './public-journey-evidence.fixture.mjs';
import { requiredCases, validateCampaign } from './staging-authority.mjs';
import { harness, line } from './staging-authority-cli.fixture.mjs';

test('six connection proofs bind to owned Launches and independent fixture context but cannot qualify the matrix', () => {
  const { results, journal, connectionFixture } = campaignFixture(requiredCases);
  validateCampaign(results, journal);
  const proofs = validateConnectionCases(results, journal);
  bindConnectionFixtures(proofs, connectionFixture, connectionFixture.provenance.nyxidCommit);
  assert.equal(proofs.length, 6);
  const missing = missingConnectionCoverage(proofs);
  assert.equal(missing.length, 20);
  assert.ok(missing.includes('provider_link/cleanup_authority'));
  assert.throws(() => requireConnectionCoverage(proofs));
});

for (const [caseId, title] of Object.entries(connectionCaseTitles)) {
  test(`full campaign refuses absent, malformed or wrongly scoped ${caseId} evidence`, () => {
    for (const update of [
      (row) => { delete row.connectionProof; },
      (row) => { row.connectionProof = null; },
      (row) => { row.connectionProof.run = 'other-run'; },
      (row) => { row.connectionProof.caseId = 'J5'; },
      (row) => { row.connectionProof.scope = 'current_image_inline'; },
      (row) => { row.connectionProof.ownerDigest = '0'.repeat(64); },
      (row) => { row.proof = row.connectionProof; delete row.connectionProof; },
    ]) {
      const { results, journal } = campaignFixture(requiredCases);
      update(results.cases.find((row) => row.case === title));
      assert.throws(() => validateCampaign(results, journal));
    }
  });
}

test('a consistent connection proof cannot change pinned provider, mode fixture or visitor authority', () => {
  for (const field of ['providerRevision', 'fixtureDigest', 'serviceDigest', 'ownerDigest']) {
    const { results, journal, connectionFixture } = campaignFixture(requiredCases);
    const proofs = validateConnectionCases(results, journal);
    const proof = proofs[0];
    for (const row of [proof, proof.before, proof.after]) row[field] = 'd'.repeat(field === 'providerRevision' ? 40 : 64);
    assert.throws(() => bindConnectionFixtures(proofs, connectionFixture, connectionFixture.provenance.nyxidCommit));
  }
});

test('two cases cannot reuse one visitor or another case run', () => {
  for (const field of ['run', 'ownerDigest']) {
    const { results, journal } = campaignFixture(requiredCases);
    const proofs = validateConnectionCases(results, journal);
    const original = proofs[1][field];
    for (const row of [proofs[1], proofs[1].before, proofs[1].after]) row[field] = proofs[0][field];
    for (const row of journal) {
      if (field === 'run' && row.run === original) row.run = proofs[0].run;
      if (field === 'ownerDigest' && row.run === proofs[1].run) row.subject = 'visitor-J4-oauth';
    }
    assert.throws(() => validateConnectionCases(results, journal));
  }
});

for (const [name, phase, mutate] of [
  ['absent proof', 'campaign_results', (h) => {
    delete h.results.cases.find((row) => row.connectionProof).connectionProof;
    h.write('results.json', h.results);
  }],
  ['absent fixture', 'connection_fixtures', (h) => {
    delete h.fixture.connectionJourneys; h.write('fixture.json', h.fixture);
  }],
  ['different reviewed provider', 'connection_fixtures', (h) => {
    h.fixture.provenance.nyxidCommit = 'd'.repeat(40); h.write('fixture.json', h.fixture);
  }],
  ['changed service fixture', 'connection_fixtures', (h) => {
    h.fixture.connectionJourneys.services.oauth.fixtureDigest = 'd'.repeat(64); h.write('fixture.json', h.fixture);
  }],
  ['unproven matrix', 'connection_coverage', () => {}],
]) test(`CLI refuses ${name} before publishing and preserves the cleanup journal`, (t) => {
  const h = harness(t);
  mutate(h);
  const before = readFileSync(h.path('cleanup.ndjson'), 'utf8');
  const result = h.run('audit');
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, line('audit', phase));
  assert.equal(readFileSync(h.path('output'), 'utf8'), '');
  assert.equal(existsSync(h.path('attestation.json')), false);
  assert.equal(readFileSync(h.path('cleanup.ndjson'), 'utf8'), before);
});

test('coverage refusal leaves an earlier artifact untouched', (t) => {
  const h = harness(t);
  h.write('attestation.json', 'earlier-artifact');
  assert.equal(h.run('audit').status, 1);
  assert.equal(readFileSync(h.path('attestation.json'), 'utf8'), 'earlier-artifact');
  assert.equal(readFileSync(h.path('output'), 'utf8'), '');
});
