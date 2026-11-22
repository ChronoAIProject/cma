import { connectionCaseTitles, identityDigest } from './connection-journey-evidence.mjs';
import { connectionProof } from './connection-journey-evidence.fixture.mjs';

export function connectionCampaignFixture(providerRevision = 'c'.repeat(40)) {
  const fixture = {
    provenance: { nyxidCommit: providerRevision },
    connectionJourneys: {
      schema: 'cma-j4-fixtures-v1',
      services: Object.fromEntries(['oauth', 'api_key', 'device_code'].map((mode) => [mode, {
        serviceId: `service-${mode}`, providerRevision, fixtureDigest: 'b'.repeat(64),
      }])),
    },
    visitors: {},
  };
  const proofs = {};
  const journal = [];
  for (const [caseId, title] of Object.entries(connectionCaseTitles)) {
    const proof = connectionProof(caseId);
    const subject = `visitor-${caseId}`;
    const id = `launch-${caseId}`;
    fixture.visitors[caseId] = { subject };
    const service = fixture.connectionJourneys.services[proof.mode];
    const binding = {
      run: `run-${caseId}`, ownerDigest: identityDigest(subject),
      targetDigest: identityDigest({ type: 'launch', id }), providerRevision,
      serviceDigest: identityDigest(service.serviceId), fixtureDigest: service.fixtureDigest,
    };
    for (const row of [proof, proof.before, proof.after]) Object.assign(row, binding);
    proof.commands.find((row) => /^\/api\/v1\/launches\//.test(row.path)).path = `/api/v1/launches/${id}/start`;
    const resource = { run: proof.run, at: new Date().toISOString(), kind: 'launches', id, subject };
    journal.push({ ...resource, action: 'created' }, { ...resource, action: 'cleanup_complete' });
    proofs[title] = proof;
  }
  return { fixture, proofs, journal };
}
