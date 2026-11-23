import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { harness, line } from '../../.github/scripts/staging-authority-cli.fixture.mjs';
import { validateCampaign } from '../../.github/scripts/staging-authority.mjs';
import { missingConnectionCoverage } from '../../.github/scripts/connection-journey-evidence.mjs';

const input = JSON.parse(readFileSync(0, 'utf8'));
const cleanup = [];
try {
  const audit = harness({ after: (callback) => cleanup.push(callback) });
  const row = input.results.cases[0];
  assert.equal(input.results.cases.length, 1);
  const proof = row.connectionProof;
  const index = audit.results.cases.findIndex((value) => value.case === row.case);
  assert.ok(index >= 0);
  const priorRun = audit.results.cases[index].connectionProof.run;
  audit.results.cases[index] = row;
  // Other campaign cases and cleanup are synthetic auditor inputs. This local
  // browser slice must never attest a real campaign or provider cleanup.
  const journal = [
    ...audit.journal.filter((entry) => entry.run !== priorRun),
    ...input.journal,
    ...input.journal
      .filter((entry) => entry.action === 'created')
      .map((entry) => ({ ...entry, action: 'cleanup_complete' })),
  ];
  audit.fixture.visitors[proof.caseId] = input.fixture.visitor;
  assert.deepEqual(audit.fixture.connectionJourneys, input.fixture.connectionJourneys);
  validateCampaign(audit.results, journal);
  assert.equal(
    missingConnectionCoverage(
      audit.results.cases.flatMap((entry) =>
        entry.connectionProof ? [entry.connectionProof] : [],
      ),
    ).length,
    20,
  );
  for (const mode of ['valid', 'missing-ownership', 'wrong-fixture']) {
    const fixture = JSON.parse(JSON.stringify(audit.fixture));
    if (mode === 'wrong-fixture')
      fixture.connectionJourneys.services[proof.mode].fixtureDigest = '0'.repeat(64);
    audit.write('fixture.json', fixture);
    const resultsBytes = JSON.stringify(audit.results);
    const journalBytes = (
      mode === 'missing-ownership' ? journal.filter((entry) => entry.run !== proof.run) : journal
    )
      .map((entry) => JSON.stringify(entry))
      .join('\n');
    audit.write('results.json', resultsBytes);
    audit.write('cleanup.ndjson', journalBytes);
    const result = audit.run('audit');
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.equal(
      result.stderr,
      line(
        'audit',
        mode === 'valid'
          ? 'connection_coverage'
          : mode === 'wrong-fixture'
            ? 'connection_fixtures'
            : 'campaign_results',
      ),
    );
    assert.equal(readFileSync(audit.path('results.json'), 'utf8'), resultsBytes);
    assert.equal(readFileSync(audit.path('cleanup.ndjson'), 'utf8'), journalBytes);
    assert.equal(readFileSync(audit.path('output'), 'utf8'), '');
    assert.equal(existsSync(audit.path('attestation.json')), false);
  }
} finally {
  for (const callback of cleanup.reverse()) callback();
}
