import { createHash, createPublicKey, verify } from 'node:crypto';

export const components = Object.freeze(['api', 'web', 'eg', 'runtime', 'runtime-omarchy']);
export const smokeCases = Object.freeze([
  'SMOKE public discovery and landing are read-only',
  'SECURITY anonymous badge sets no cookie and references no credentials',
  'SMOKE anonymous console stays at the passive sign-in gate',
]);
export const limit = 65_536;
export const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const sha = /^[0-9a-f]{64}$/;
const revision = /^[0-9a-f]{40}$/;
const digest = /^sha256:[0-9a-f]{64}$/;
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const source = 'https://github.com/ChronoAIProject/cma';
const reasons = new Set([
  'invalid_input', 'untrusted_policy', 'invalid_signature', 'context_mismatch',
  'observer_mismatch', 'identity_mismatch', 'scope_incomplete', 'receipt_replay',
  'window_invalid', 'artifact_mismatch', 'cases_incomplete', 'smoke_failed',
]);
class Refusal extends Error {}
function requireValue(condition, code = 'invalid_input') {
  if (!condition) throw new Refusal(code);
}
function keys(value, names) {
  requireValue(value !== null && typeof value === 'object' && !Array.isArray(value));
  const actual = Object.keys(value).sort();
  requireValue(JSON.stringify(actual) === JSON.stringify([...names].sort()));
}
function matches(value, pattern) {
  requireValue(typeof value === 'string' && pattern.test(value));
}
function parse(bytes) {
  requireValue(Buffer.isBuffer(bytes) && bytes.length > 0 && bytes.length <= limit);
  const text = bytes.toString('utf8');
  requireValue(Buffer.from(text).equals(bytes));
  return JSON.parse(text);
}
function timestamp(value) {
  requireValue(typeof value === 'string' && value.length === 24);
  const time = Date.parse(value);
  requireValue(Number.isFinite(time) && new Date(time).toISOString() === value);
  return time;
}
function context(value) {
  keys(value, ['runId', 'attempt', 'event', 'checkout', 'targetHash', 'originHash', 'manifestHash']);
  matches(value.runId, /^[1-9][0-9]{0,19}$/);
  requireValue(value.attempt === 1 && value.event === 'schedule', 'context_mismatch');
  matches(value.checkout, revision);
  for (const field of ['targetHash', 'originHash', 'manifestHash']) matches(value[field], sha);
}
function sameContext(actual, expected) {
  context(actual);
  requireValue(Object.keys(expected).every((key) => actual[key] === expected[key]), 'context_mismatch');
}
function publicKey(encoded) {
  matches(encoded, /^[A-Za-z0-9+/]{59}=$/);
  const der = Buffer.from(encoded, 'base64');
  requireValue(der.toString('base64') === encoded, 'untrusted_policy');
  const key = createPublicKey({ key: der, type: 'spki', format: 'der' });
  requireValue(key.asymmetricKeyType === 'ed25519', 'untrusted_policy');
  return key;
}
function authenticated(bytes, key) {
  const envelope = parse(bytes);
  keys(envelope, ['payload', 'signature']);
  requireValue(typeof envelope.payload === 'string' && envelope.payload.length <= limit);
  matches(envelope.signature, /^[A-Za-z0-9+/]{86}==$/);
  const signature = Buffer.from(envelope.signature, 'base64');
  requireValue(signature.toString('base64') === envelope.signature &&
    verify(null, Buffer.from(envelope.payload), key, signature), 'invalid_signature');
  return parse(Buffer.from(envelope.payload));
}
function policy(bytes, pin) {
  matches(pin, sha);
  requireValue(hash(bytes) === pin, 'untrusted_policy');
  const value = parse(bytes);
  keys(value, ['schema', 'context', 'observerKey', 'harnessKey', 'observerHash', 'observerConfigHash', 'maxWindowMs', 'maxAgeMs']);
  requireValue(value.schema === 'cma-smoke-trust-v1', 'untrusted_policy');
  context(value.context);
  for (const field of ['observerHash', 'observerConfigHash']) matches(value[field], sha);
  requireValue(value.observerKey !== value.harnessKey, 'untrusted_policy');
  requireValue(Number.isSafeInteger(value.maxWindowMs) && value.maxWindowMs > 0 && value.maxWindowMs <= 900_000);
  requireValue(Number.isSafeInteger(value.maxAgeMs) && value.maxAgeMs > 0 && value.maxAgeMs <= 300_000);
  return { value, observer: publicKey(value.observerKey), harness: publicKey(value.harnessKey) };
}
const identityFields = ['source', 'revision', 'reference', 'rootDigest', 'platformDigest', 'configDigest', 'configurationHash', 'routeHash'];
function identity(row, name) {
  requireValue(row.source === source, 'identity_mismatch');
  matches(row.revision, revision);
  matches(row.reference, /^[a-z0-9.-]+(?::[0-9]{1,5})?\/[a-z0-9/_.-]+@sha256:[0-9a-f]{64}$/);
  requireValue(row.reference.length <= 256);
  for (const field of ['rootDigest', 'platformDigest', 'configDigest']) matches(row[field], digest);
  requireValue(row.reference.endsWith(`@${row.rootDigest}`), 'identity_mismatch');
  matches(row.configurationHash, sha);
  if (name === 'api' || name === 'web') matches(row.routeHash, sha);
  else requireValue(row.routeHash === null);
}
function manifest(bytes, expected) {
  requireValue(hash(bytes) === expected.manifestHash, 'artifact_mismatch');
  const value = parse(bytes);
  keys(value, ['schema', 'target', 'origin', 'components']);
  requireValue(value.schema === 'cma-smoke-candidate-v1');
  matches(value.target, /^[a-z0-9][a-z0-9_-]{0,63}$/);
  requireValue(typeof value.origin === 'string' && value.origin.length <= 256);
  const origin = new URL(value.origin);
  requireValue(origin.protocol === 'https:' && origin.origin === value.origin);
  requireValue(hash(value.target) === expected.targetHash && hash(value.origin) === expected.originHash, 'context_mismatch');
  keys(value.components, components);
  for (const name of components) {
    const row = value.components[name];
    keys(row, [...identityFields, 'required', 'proof']);
    identity(row, name);
    requireValue(typeof row.required === 'boolean');
    requireValue(['running-cohort', 'configured-pin'].includes(row.proof));
    if (name === 'api' || name === 'web') requireValue(row.required && row.proof === 'running-cohort', 'scope_incomplete');
    if (row.proof === 'configured-pin') requireValue(name.startsWith('runtime'));
  }
  return value;
}
function observedComponents(rows, expected) {
  requireValue(rows !== null && typeof rows === 'object' && !Array.isArray(rows));
  requireValue(Object.keys(rows).every((name) => components.includes(name)));
  const summary = [];
  for (const name of components) {
    const wanted = expected[name], row = rows[name];
    if (row === undefined) {
      requireValue(!wanted.required, 'scope_incomplete');
      summary.push({ component: name, required: false, observed: false, matched: false, proof: 'not-observed' });
      continue;
    }
    keys(row, [...identityFields, 'proof', 'cohortHash', 'instances', 'runningDigest']);
    identity(row, name);
    requireValue(identityFields.every((field) => row[field] === wanted[field]), 'identity_mismatch');
    requireValue(['running-cohort', 'configured-pin'].includes(row.proof));
    requireValue(wanted.proof === row.proof || (wanted.proof === 'configured-pin' && row.proof === 'running-cohort'), 'scope_incomplete');
    if (row.proof === 'running-cohort') {
      matches(row.cohortHash, sha);
      requireValue(Number.isSafeInteger(row.instances) && row.instances > 0 && row.instances <= 10_000);
      requireValue(row.runningDigest === row.platformDigest, 'identity_mismatch');
    } else {
      requireValue(name.startsWith('runtime') && row.cohortHash === null && row.instances === 0 && row.runningDigest === null, 'scope_incomplete');
    }
    summary.push({ component: name, required: wanted.required, observed: true, matched: true,
      proof: row.proof, revision: row.revision, rootDigest: row.rootDigest,
      platformDigest: row.platformDigest, configDigest: row.configDigest,
      configurationHash: row.configurationHash, instances: row.instances });
  }
  return summary;
}
function receipt(bytes, key, trust, expected, phase) {
  const value = authenticated(bytes, key);
  keys(value, ['schema', 'context', 'phase', 'nonce', 'observedAt', 'observerHash', 'observerConfigHash', 'executionHash', 'resultsHash', 'components']);
  requireValue(value.schema === 'cma-smoke-observation-v1' && value.phase === phase);
  sameContext(value.context, trust.context);
  matches(value.nonce, uuid);
  timestamp(value.observedAt);
  requireValue(value.observerHash === trust.observerHash && value.observerConfigHash === trust.observerConfigHash, 'observer_mismatch');
  if (phase === 'before') requireValue(value.executionHash === null && value.resultsHash === null);
  else { matches(value.executionHash, sha); matches(value.resultsHash, sha); }
  observedComponents(value.components, expected.components);
  return value;
}
function results(bytes) {
  const value = parse(bytes);
  keys(value, ['status', 'cases']);
  requireValue(['passed', 'failed', 'timedout', 'interrupted'].includes(value.status));
  requireValue(Array.isArray(value.cases) && value.cases.length === smokeCases.length, 'cases_incomplete');
  const names = new Set();
  for (const row of value.cases) {
    keys(row, ['case', 'status', 'durationMs']);
    requireValue(smokeCases.includes(row.case) && !names.has(row.case), 'cases_incomplete');
    names.add(row.case);
    requireValue(['passed', 'failed', 'timedOut', 'skipped', 'interrupted'].includes(row.status));
    requireValue(Number.isFinite(row.durationMs) && row.durationMs >= 0 && row.durationMs <= 900_000);
  }
  return value;
}

export function rawStatus(bytes) {
  try {
    const value = parse(bytes);
    return ['passed', 'failed', 'timedout', 'interrupted'].includes(value?.status) ? value.status : 'unavailable';
  } catch { return 'unavailable'; }
}
export function unavailable(reason = 'binding_disabled', rawSmokeStatus = 'unavailable') {
  return { schema: 'cma-smoke-qualification-v1', rawSmokeStatus, qualification: 'unavailable', reason };
}

// The caller obtains policyHash from protected release authority, never from the evidence bundle.
// Observer signatures authenticate observations; the independent harness signs the actual execution.
export function qualifySmoke(input, now = Date.now()) {
  const rawSmokeStatus = rawStatus(input?.resultsBytes);
  try {
    const result = results(input.resultsBytes);
    const { value: trust, observer, harness } = policy(input.policyBytes, input.policyHash);
    const expected = manifest(input.manifestBytes, trust.context);
    const before = receipt(input.beforeBytes, observer, trust, expected, 'before');
    const after = receipt(input.afterBytes, observer, trust, expected, 'after');
    const execution = authenticated(input.executionBytes, harness);
    keys(execution, ['schema', 'context', 'beforeHash', 'beforeNonce', 'afterNonce', 'startedAt', 'finishedAt', 'resultsHash']);
    requireValue(execution.schema === 'cma-smoke-execution-v1');
    sameContext(execution.context, trust.context);
    requireValue(execution.beforeHash === hash(input.beforeBytes) && after.executionHash === hash(input.executionBytes), 'artifact_mismatch');
    requireValue(execution.resultsHash === hash(input.resultsBytes) && after.resultsHash === execution.resultsHash, 'artifact_mismatch');
    requireValue(before.nonce !== after.nonce && execution.beforeNonce === before.nonce && execution.afterNonce === after.nonce, 'receipt_replay');
    const start = timestamp(execution.startedAt), end = timestamp(execution.finishedAt);
    const first = timestamp(before.observedAt), last = timestamp(after.observedAt);
    requireValue(Number.isSafeInteger(now) && first <= start && start <= end && end <= last && last <= now &&
      last - first <= trust.maxWindowMs && now - last <= trust.maxAgeMs, 'window_invalid');
    requireValue(result.cases.reduce((sum, row) => sum + row.durationMs, 0) <= end - start, 'window_invalid');
    requireValue(components.every((name) => {
      const left = before.components[name], right = after.components[name];
      return left === undefined ? right === undefined : right !== undefined && Object.keys(left).every((field) => left[field] === right[field]);
    }), 'identity_mismatch');
    requireValue(result.status === 'passed' && result.cases.every((row) => row.status === 'passed'), 'smoke_failed');
    return { schema: 'cma-smoke-qualification-v1', rawSmokeStatus, qualification: 'verified', reason: 'matched',
      scope: 'declared-smoke-components-only', continuousObservation: false,
      context: trust.context, resultsHash: execution.resultsHash, executionHash: after.executionHash,
      beforeHash: hash(input.beforeBytes), afterHash: hash(input.afterBytes),
      trust: { policyHash: input.policyHash, observerKeyHash: hash(trust.observerKey), harnessKeyHash: hash(trust.harnessKey),
        observerHash: trust.observerHash, observerConfigHash: trust.observerConfigHash },
      window: { before: before.observedAt, start: execution.startedAt, end: execution.finishedAt, after: after.observedAt },
      cases: smokeCases.length, components: observedComponents(after.components, expected.components) };
  } catch (error) {
    return { schema: 'cma-smoke-qualification-v1', rawSmokeStatus, qualification: 'refused',
      reason: error instanceof Refusal && reasons.has(error.message) ? error.message : 'invalid_input' };
  }
}
