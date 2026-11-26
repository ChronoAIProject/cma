import assert from 'node:assert/strict';
import {
  assertConnectionProof, connectionCaseTitles, identityDigest, missingConnectionCoverage,
} from './connection-journey-evidence.mjs';

export function validateConnectionCases(results, journal) {
  const runs = new Set();
  const owners = new Set();
  return Object.entries(connectionCaseTitles).map(([caseId, title]) => {
    const rows = results.cases.filter((row) => row.case === title);
    assert.equal(rows.length, 1);
    const proof = rows[0].connectionProof;
    assert.ok(proof && typeof proof === 'object');
    const launches = journal.filter((row) => row.action === 'created' && row.kind === 'launches' &&
      row.run === proof.run && identityDigest(row.subject) === proof.ownerDigest &&
      identityDigest({ type: 'launch', id: row.id }) === proof.targetDigest);
    assert.equal(launches.length, 1, 'Connection proof requires its owned Launch');
    const verified = assertConnectionProof(proof, {
      run: launches[0].run, caseId, providerRevision: proof.providerRevision,
    });
    assert.ok(!runs.has(verified.run), 'Connection cases require separate runs');
    assert.ok(!owners.has(verified.ownerDigest), 'Connection cases require isolated visitors');
    runs.add(verified.run);
    owners.add(verified.ownerDigest);
    return verified;
  });
}

export function bindConnectionFixtures(proofs, fixture, providerRevision) {
  assert.match(providerRevision, /^[a-f0-9]{40}$/);
  assert.equal(fixture.provenance.nyxidCommit, providerRevision);
  const manifest = fixture.connectionJourneys;
  assert.deepEqual(Object.keys(manifest).sort(), ['schema', 'services']);
  assert.equal(manifest.schema, 'cma-j4-fixtures-v1');
  assert.deepEqual(Object.keys(manifest.services).sort(), ['api_key', 'device_code', 'oauth']);
  const services = new Set();
  for (const row of Object.values(manifest.services)) {
    assert.deepEqual(Object.keys(row).sort(), ['fixtureDigest', 'providerRevision', 'serviceId']);
    assert.match(row.serviceId, /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/);
    assert.match(row.fixtureDigest, /^[a-f0-9]{64}$/);
    assert.equal(row.providerRevision, providerRevision);
    assert.ok(!services.has(row.serviceId));
    services.add(row.serviceId);
  }
  for (const proof of proofs) {
    const expected = manifest.services[proof.mode];
    assertConnectionProof(proof, { run: proof.run, caseId: proof.caseId, providerRevision });
    assert.equal(proof.fixtureDigest, expected.fixtureDigest);
    assert.equal(proof.serviceDigest, identityDigest(expected.serviceId));
    assert.equal(proof.ownerDigest, identityDigest(fixture.visitors[proof.caseId].subject));
  }
}

export function requireConnectionCoverage(proofs) {
  assert.deepEqual(missingConnectionCoverage(proofs), [], 'Connection outcome coverage remains incomplete');
}
