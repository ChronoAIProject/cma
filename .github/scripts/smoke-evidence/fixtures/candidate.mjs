import { generateKeyPairSync, sign } from 'node:crypto';
import { components, hash, smokeCases } from '../contract.mjs';

const json = (value) => Buffer.from(JSON.stringify(value));
const sha = (char) => char.repeat(64);
const keys = () => generateKeyPairSync('ed25519');
const publicBytes = (key) => key.export({ format: 'der', type: 'spki' }).toString('base64');
function signed(value, key) {
  const payload = JSON.stringify(value);
  return json({ payload, signature: sign(null, Buffer.from(payload), key).toString('base64') });
}

// Keys are generated per synthetic fixture and never written or used by an observer.
export function fixture(now = Date.now()) {
  const observer = keys(), harness = keys();
  const time = (offset) => new Date(now + offset).toISOString();
  const manifest = { schema: 'cma-smoke-candidate-v1', target: 'synthetic', origin: 'https://smoke.example.test',
    components: Object.fromEntries(components.map((name, index) => [name, {
      source: 'https://github.com/ChronoAIProject/cma', revision: String(index + 1).repeat(40),
      reference: `registry.example.test/cma-${name}@sha256:${sha('a')}`,
      rootDigest: `sha256:${sha('a')}`, platformDigest: `sha256:${sha('b')}`, configDigest: `sha256:${sha('c')}`,
      configurationHash: sha('d'), routeHash: index < 2 ? sha('e') : null, required: true, proof: 'running-cohort',
    }])) };
  const context = { runId: '12345', attempt: 1, event: 'schedule', checkout: 'f'.repeat(40),
    originHash: hash(manifest.origin), targetHash: hash(manifest.target), manifestHash: hash(json(manifest)) };
  const policy = { schema: 'cma-smoke-trust-v1', context, observerKey: publicBytes(observer.publicKey),
    harnessKey: publicBytes(harness.publicKey), observerHash: sha('1'), observerConfigHash: sha('2'), maxWindowMs: 900_000, maxAgeMs: 300_000 };
  const observed = Object.fromEntries(components.map((name) => {
    const { required, ...row } = manifest.components[name];
    return [name, { ...row, cohortHash: sha('3'), instances: 2, runningDigest: row.platformDigest }];
  }));
  const before = { schema: 'cma-smoke-observation-v1', context: structuredClone(context), phase: 'before',
    nonce: '00000000-0000-4000-8000-000000000001', observedAt: time(-12_000),
    observerHash: policy.observerHash, observerConfigHash: policy.observerConfigHash,
    executionHash: null, resultsHash: null, components: observed };
  const after = { ...structuredClone(before), phase: 'after', nonce: '00000000-0000-4000-8000-000000000002',
    observedAt: time(-1000), executionHash: sha('0'), resultsHash: sha('0') };
  const execution = { schema: 'cma-smoke-execution-v1', context: structuredClone(context), beforeHash: sha('0'),
    beforeNonce: before.nonce, afterNonce: after.nonce, startedAt: time(-10_000), finishedAt: time(-2000), resultsHash: sha('0') };
  const results = { status: 'passed', cases: smokeCases.map((name) => ({ case: name, status: 'passed', durationMs: 1000 })) };
  const state = { policy, manifest, before, after, execution, results, now,
    signObserver: (value) => signed(value, observer.privateKey), signHarness: (value) => signed(value, harness.privateKey) };
  state.encode = () => {
    const manifestBytes = json(state.manifest), resultsBytes = json(state.results);
    const beforeBytes = state.signObserver(state.before);
    const executionBytes = state.signHarness({ ...state.execution, beforeHash: hash(beforeBytes), resultsHash: hash(resultsBytes) });
    const afterBytes = state.signObserver({ ...state.after, executionHash: hash(executionBytes), resultsHash: hash(resultsBytes) });
    const policyBytes = json(state.policy);
    return { policyBytes, policyHash: hash(policyBytes), manifestBytes, resultsBytes, beforeBytes, executionBytes, afterBytes };
  };
  state.rebindManifest = () => {
    const manifestHash = hash(json(state.manifest));
    for (const value of [state.policy, state.before, state.after, state.execution]) value.context.manifestHash = manifestHash;
  };
  return state;
}
