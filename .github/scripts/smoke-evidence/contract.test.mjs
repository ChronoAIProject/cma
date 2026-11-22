import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { components, hash, qualifySmoke, smokeCases } from './contract.mjs';
import { fixture } from './fixtures/candidate.mjs';

const run = (f) => qualifySmoke(f.encode(), f.now);
function refused(f, reason) {
  const result = run(f);
  assert.equal(result.qualification, 'refused');
  assert.equal(result.reason, reason);
  return result;
}
test('independently signed component versions qualify only the declared smoke scope', () => {
  const f = fixture(), result = run(f);
  assert.equal(result.qualification, 'verified');
  assert.equal(result.cases, 3);
  assert.equal(result.scope, 'declared-smoke-components-only');
  assert.equal(result.continuousObservation, false);
  assert.equal(new Set(result.components.map((row) => row.revision)).size, 5);
  assert.ok(result.components.every((row) => row.revision !== result.context.checkout));
  assert.equal(result.trust.policyHash, f.encode().policyHash);
  assert.equal(result.resultsHash, hash(f.encode().resultsBytes));
  assert.equal(result.executionHash, hash(f.encode().executionBytes));
  assert.ok(!JSON.stringify(result).includes('example.test'));
});
test('smoke inventory matches the actual three scheduled cases, without arbitrary passing substitutes', () => {
  const source = readFileSync(new URL('../../../web/e2e/smoke.pw.ts', import.meta.url), 'utf8');
  const names = [...source.matchAll(/\btest\('([^']+)'/g)].map((match) => match[1]);
  assert.deepEqual(names, smokeCases);
});
for (const component of components) {
  for (const field of ['source', 'revision', 'rootDigest', 'platformDigest', 'configDigest', 'configurationHash', 'reference', 'routeHash']) {
    if (field === 'routeHash' && !['api', 'web'].includes(component)) continue;
    test(`${component} ${field} must match the reviewed manifest`, () => {
      const f = fixture();
      for (const receipt of [f.before, f.after]) {
        const row = receipt.components[component];
        row[field] = field === 'source' ? 'https://example.test/untrusted' : field === 'revision' ? '9'.repeat(40) : field.endsWith('Hash') ? '9'.repeat(64) :
          field === 'reference' ? `other.example.test/image@${row.rootDigest}` : `sha256:${'9'.repeat(64)}`;
      }
      refused(f, 'identity_mismatch');
    });
  }
  test(`missing required ${component} fails even when the smoke passed`, () => {
    const f = fixture(); delete f.after.components[component];
    assert.equal(refused(f, 'scope_incomplete').rawSmokeStatus, 'passed');
  });
}
test('optional unobserved components remain explicitly uncovered', () => {
  const f = fixture();
  for (const name of ['eg', 'runtime', 'runtime-omarchy']) {
    f.manifest.components[name].required = false;
    delete f.before.components[name]; delete f.after.components[name];
  }
  f.rebindManifest();
  const result = run(f);
  assert.equal(result.qualification, 'verified');
  assert.deepEqual(result.components.filter((row) => !row.observed).map((row) => row.component), ['eg', 'runtime', 'runtime-omarchy']);
  assert.ok(result.components.filter((row) => !row.observed).every((row) => row.proof === 'not-observed' && !row.matched));
});
test('a configured pin cannot impersonate a running runtime cohort', () => {
  const f = fixture();
  for (const value of [f.before, f.after]) Object.assign(value.components.runtime, {
    proof: 'configured-pin', cohortHash: null, instances: 0, runningDigest: null,
  });
  refused(f, 'scope_incomplete');
  f.manifest.components.runtime.proof = 'configured-pin'; f.rebindManifest();
  const result = run(f);
  assert.equal(result.qualification, 'verified');
  assert.equal(result.components.find((row) => row.component === 'runtime').proof, 'configured-pin');
});
for (const mutate of [
  (f) => { f.after.components.api.runningDigest = f.after.components.api.configDigest; },
  (f) => { f.after.components.api.instances = 3; },
  (f) => { f.after.components.api.cohortHash = '9'.repeat(64); },
  (f) => { f.after.components.web.configurationHash = '9'.repeat(64); },
  (f) => { f.after.components.web.routeHash = '9'.repeat(64); },
]) test('changed instance, routing or effective configuration refuses qualification', () => {
  const f = fixture(); mutate(f); refused(f, 'identity_mismatch');
});
for (const part of ['before', 'after', 'execution']) {
  for (const field of ['runId', 'attempt', 'checkout', 'targetHash', 'originHash', 'manifestHash']) {
    test(`${part} cannot replay another ${field}`, () => {
      const f = fixture(); f[part].context[field] = field === 'attempt' ? 2 : field === 'runId' ? '99999' : '9'.repeat(field === 'checkout' ? 40 : 64);
      refused(f, 'context_mismatch');
    });
  }
}
for (const [label, mutate, reason] of [
  ['rerun', (f) => { f.policy.context.attempt = 2; }, 'context_mismatch'],
  ['manual dispatch', (f) => { f.policy.context.event = 'workflow_dispatch'; }, 'context_mismatch'],
  ['same observer and harness key', (f) => { f.policy.harnessKey = f.policy.observerKey; }, 'untrusted_policy'],
  ['observer tool differs', (f) => { f.before.observerHash = '9'.repeat(64); }, 'observer_mismatch'],
  ['observer config differs', (f) => { f.after.observerConfigHash = '9'.repeat(64); }, 'observer_mismatch'],
  ['nonce reused', (f) => { f.after.nonce = f.before.nonce; f.execution.afterNonce = f.before.nonce; }, 'receipt_replay'],
  ['unexpected challenge', (f) => { f.execution.beforeNonce = f.after.nonce; }, 'receipt_replay'],
  ['future observation', (f) => { f.after.observedAt = new Date(f.now + 1).toISOString(); }, 'window_invalid'],
  ['stale observation', (f) => { f.now += 300_001; }, 'window_invalid'],
  ['observation after start', (f) => { f.before.observedAt = f.execution.finishedAt; }, 'window_invalid'],
  ['observation before finish', (f) => { f.after.observedAt = f.execution.startedAt; }, 'window_invalid'],
  ['reversed execution', (f) => { [f.execution.startedAt, f.execution.finishedAt] = [f.execution.finishedAt, f.execution.startedAt]; }, 'window_invalid'],
  ['excessive window', (f) => { f.before.observedAt = new Date(Date.parse(f.after.observedAt) - 900_001).toISOString(); }, 'window_invalid'],
  ['case duration outside execution', (f) => { f.results.cases[0].durationMs = 9000; }, 'window_invalid'],
  ['missing case', (f) => { f.results.cases.pop(); }, 'cases_incomplete'],
  ['duplicate case', (f) => { f.results.cases[1] = f.results.cases[0]; }, 'cases_incomplete'],
  ['substitute case', (f) => { f.results.cases[0].case = 'unrelated passing test'; }, 'cases_incomplete'],
  ['extra retry result', (f) => { f.results.cases.push(f.results.cases[0]); }, 'cases_incomplete'],
  ['failed case', (f) => { f.results.cases[0].status = 'failed'; }, 'smoke_failed'],
  ['skipped case', (f) => { f.results.cases[0].status = 'skipped'; }, 'smoke_failed'],
  ['failed run', (f) => { f.results.status = 'failed'; }, 'smoke_failed'],
]) test(`rejects ${label}`, () => { const f = fixture(); mutate(f); refused(f, reason); });

for (const field of ['beforeBytes', 'afterBytes', 'executionBytes']) {
  test(`operator JSON and unauthenticated ${field} cannot qualify`, () => {
    const f = fixture(), input = f.encode();
    input[field] = Buffer.from(JSON.stringify({ ...JSON.parse(input[field]), signature: 'A'.repeat(86) + '==' }));
    assert.equal(qualifySmoke(input, f.now).reason, 'invalid_signature');
    input[field] = Buffer.from(JSON.stringify(f.before));
    assert.equal(qualifySmoke(input, f.now).qualification, 'refused');
  });
}
test('a changed manifest or trust policy requires the independently supplied pin', () => {
  const f = fixture(), input = f.encode();
  assert.equal(qualifySmoke({ ...input, policyHash: '0'.repeat(64) }, f.now).reason, 'untrusted_policy');
  assert.equal(qualifySmoke({ ...input, manifestBytes: Buffer.concat([input.manifestBytes, Buffer.from(' ')]) }, f.now).reason, 'artifact_mismatch');
});
test('the observer cannot sign harness execution and the harness cannot sign observations', () => {
  const f = fixture(), input = f.encode();
  input.executionBytes = f.signObserver(f.execution);
  assert.equal(qualifySmoke(input, f.now).reason, 'invalid_signature');
  input.beforeBytes = f.signHarness(f.before);
  assert.equal(qualifySmoke(input, f.now).reason, 'invalid_signature');
});
test('running coverage requires real instance counts and API/Web route fingerprints', () => {
  for (const name of components) {
    const f = fixture(); f.before.components[name].instances = 0;
    refused(f, 'invalid_input');
  }
  for (const name of ['api', 'web']) {
    const f = fixture(); f.before.components[name].routeHash = null;
    refused(f, 'invalid_input');
    f.manifest.components[name].required = false; f.rebindManifest();
    refused(f, 'scope_incomplete');
  }
});
test('every component needs an explicit manifest scope and recognized schemas', () => {
  for (const name of components) {
    const f = fixture(); delete f.manifest.components[name]; f.rebindManifest();
    refused(f, 'invalid_input');
  }
  for (const part of ['policy', 'manifest', 'before', 'after', 'execution']) {
    const f = fixture(); f[part].schema = 'unknown'; f.rebindManifest();
    assert.equal(run(f).qualification, 'refused');
  }
});
test('window and freshness boundaries are inclusive but cannot be relaxed by policy', () => {
  const f = fixture();
  f.before.observedAt = new Date(Date.parse(f.after.observedAt) - 900_000).toISOString();
  f.now = Date.parse(f.after.observedAt) + 300_000;
  assert.equal(run(f).qualification, 'verified');
  for (const [field, invalid] of [['maxWindowMs', 900_001], ['maxAgeMs', 300_001], ['maxWindowMs', 0], ['maxAgeMs', -1]]) {
    const altered = fixture(); altered.policy[field] = invalid;
    refused(altered, 'invalid_input');
  }
});
test('incomplete or failed cases retain the reported smoke status separately from qualification', () => {
  const f = fixture(); f.results.cases.pop();
  assert.equal(refused(f, 'cases_incomplete').rawSmokeStatus, 'passed');
  f.results.status = 'failed';
  assert.equal(refused(f, 'cases_incomplete').rawSmokeStatus, 'failed');
});
test('raw result bytes and authenticated before/execution artifacts are bound, not just JSON values', () => {
  const f = fixture(), input = f.encode();
  for (const field of ['resultsBytes', 'beforeBytes', 'executionBytes']) {
    assert.equal(qualifySmoke({ ...input, [field]: Buffer.concat([input[field], Buffer.from(' ')]) }, f.now).reason, 'artifact_mismatch');
  }
});
test('unapproved fields and oversized or malformed inputs never echo a sensitive canary', () => {
  const canary = 'private-observation-canary-do-not-publish';
  for (const part of ['policy', 'manifest', 'before', 'after', 'execution', 'results']) {
    const f = fixture(); f[part].secret = canary;
    assert.equal(run(f).qualification, 'refused');
    assert.ok(!JSON.stringify(run(f)).includes(canary));
  }
  const f = fixture();
  for (const field of ['policyBytes', 'manifestBytes', 'beforeBytes', 'afterBytes', 'executionBytes', 'resultsBytes']) {
    for (const value of [canary, canary.repeat(5000)]) {
      const result = qualifySmoke({ ...f.encode(), [field]: Buffer.from(value) }, f.now);
      assert.equal(result.qualification, 'refused');
      assert.ok(!JSON.stringify(result).includes(canary));
    }
  }
});
