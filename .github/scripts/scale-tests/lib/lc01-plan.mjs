import { check, hash, isHash } from './checks.mjs';

export const minute = 60_000;
export const warmEnd = 10 * minute;
export const steadyStart = 20 * minute;
export const steadyEnd = 80 * minute;
export const end = 90 * minute;
export const turnDeadlineMs = 120_000;
export const protocols = Object.freeze(['rest', 'agui', 'responses']);
const mix = ['rest', 'rest', 'rest', 'rest', 'agui', 'agui', 'agui', 'responses', 'responses', 'responses'];
const agentId = /^agt_[A-Za-z0-9_-]{1,120}$/;
const sandboxId = /^sbx_[A-Za-z0-9_-]{1,120}$/;
const credential = /^[\x21-\x7e]{16,4096}$/;
export const fixturePath = 'lc01-fixture.txt';

function origin(value) {
  try {
    const parsed = new URL(value);
    return parsed.protocol === 'https:' && value === parsed.origin;
  } catch {
    return false;
  }
}

export function phase(at) {
  if (at < warmEnd) return 'warm';
  if (at < steadyStart) return 'ramp';
  if (at < steadyEnd) return 'steady';
  return 'down';
}

export function activeWindow(index) {
  return index < 25
    ? { open: 0, close: end }
    : { open: warmEnd + (index - 25) * 24_000, close: steadyEnd + (50 - index) * 24_000 };
}

export function loadPlan() {
  const entries = [];
  for (let user = 0; user < 50; user += 1) {
    const { open, close } = activeWindow(user);
    entries.push({ at: open, kind: 'connect', user }, { at: close, kind: 'disconnect', user });
    for (let at = user * 6_000; at < close; at += 5 * minute) {
      if (at >= open) entries.push({ at, kind: 'turn', user, protocol: mix[user % mix.length] });
    }
  }
  const steadyReads = 5 * 60 * 60;
  for (let read = 0; read < steadyReads; read += 1) {
    entries.push({ at: steadyStart + read * 200, kind: 'read', user: read % 50 });
  }
  for (let at = steadyStart; at < steadyEnd; at += 10 * minute) {
    for (let user = 0; user < 50; user += 1) entries.push({ at, kind: 'burst', user });
  }
  return entries.sort((a, b) => a.at - b.at);
}

function distinct(values) {
  return new Set(values).size === values.length;
}

export function validateConfig(config) {
  check(config?.schema === 'cma-lc01-staging-v1' && config.reservation?.isolated === true,
    'isolated_staging_reservation_required', true);
  check(origin(config.origin), 'staging_origin_required', true);
  check(/^[a-f0-9]{40}$/.test(config.candidate?.sourceSha)
    && /^sha256:[a-f0-9]{64}$/.test(config.candidate?.imageDigest), 'candidate_pin_required', true);
  for (const field of ['cohortHash', 'backgroundPeakHash', 'placementHash', 'runtimeMappingHash', 'providerBudgetHash']) {
    check(isHash(config.reservation[field]), 'reservation_evidence_required', true);
  }
  check(isHash(config.fixtureSha256) && Number.isSafeInteger(config.reservation.verifiedK)
    && config.reservation.verifiedK >= 50 && config.reservation.nPlusOneVerified === true,
  'fixture_and_verified_capacity_required', true);
  check(Array.isArray(config.users) && config.users.length === 50
    && config.users.every((user) => isHash(user?.subjectHash) && agentId.test(user.agent)
      && sandboxId.test(user.sandbox) && credential.test(user.bearer)
      && (user.providerClass === 'funded' || user.providerClass === 'deterministic')
      && isHash(user.runtimeEvidenceHash)
      && Number.isSafeInteger(user.delaySeconds) && user.delaySeconds >= 10 && user.delaySeconds <= 30
      && typeof user.modelClass === 'string' && /^[a-z0-9_-]{1,40}$/.test(user.modelClass)),
  'fifty_registered_ready_owners_required', true);
  for (const field of ['subjectHash', 'agent', 'sandbox', 'bearer']) {
    check(distinct(config.users.map((user) => user[field])), 'owner_fixture_reused', true);
  }
  for (const protocol of protocols) {
    const owners = config.users.filter((_, index) => mix[index % mix.length] === protocol);
    const funded = owners.filter((user) => user.providerClass === 'funded').length;
    check(funded >= owners.length / 10 && funded < owners.length, 'funded_cohort_missing', true);
  }
  check(Array.isArray(config.background) && config.background.length === 2
    && config.background.some((row) => row.consumer === 'nyxid')
    && config.background.some((row) => row.consumer === 'direct-api')
    && config.background.every((row) => agentId.test(row.agent) && credential.test(row.bearer)
      && origin(row.origin) && typeof row.prefix === 'string'
      && /^(?:\/[A-Za-z0-9_-]+)*$/.test(row.prefix)
      && Number.isFinite(row.peakReadsPerSecond) && row.peakReadsPerSecond > 0
      && row.peakReadsPerSecond <= 1000)
    && config.background.find((row) => row.consumer === 'nyxid').prefix.length > 0,
  'measured_background_peak_required', true);
  const all = [...config.users, ...config.background];
  check(distinct(all.map((row) => row.bearer)) && distinct(all.map((row) => row.agent)),
    'background_fixture_reused', true);
  return config;
}

export function turnFixture(run, user, at, delaySeconds, fixtureSha256) {
  const nonce = hash(`${run}/${user}/${at}`).slice(0, 32);
  const outputPath = `lc01-${nonce}.txt`;
  const expectedHash = hash(`${nonce}\n`);
  const input = `Perform this fixed qualification task using at most two tool calls. Read ${fixturePath} `
    + `and confirm its SHA-256 is ${fixtureSha256}. Wait ${delaySeconds} seconds. Write exactly ${nonce} `
    + `followed by one newline to the new file ${outputPath}. Read it back, compute its SHA-256, `
    + 'and report the SHA-256 in the final response. Leave other files alone.';
  return { nonce, key: `lc01-${nonce}`, outputPath, expectedHash, input };
}

export function workloadShape(config) {
  return hash(JSON.stringify({
    sourceSha: config.candidate.sourceSha, imageDigest: config.candidate.imageDigest,
    fixtureSha256: config.fixtureSha256,
    users: config.users.map(({ providerClass, modelClass, delaySeconds }) => [providerClass, modelClass, delaySeconds]),
    background: config.background.map(({ consumer, peakReadsPerSecond }) => [consumer, peakReadsPerSecond]),
  }));
}
