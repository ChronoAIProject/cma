import { lstat, readFile, realpath } from 'node:fs/promises';
import { isAbsolute, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { check, hash, isHash } from '../lib/checks.mjs';

export const ROOT = fileURLToPath(new URL('../../../../', import.meta.url));
export const SETUP_MS = 120000;
export const TURN_MS = 60000;
export const FIRST_VISIBLE_MS = 10000;
export const SETTLE_MS = 120000;
export const CLEANUP_MS = 30 * 60000;
export const BODY_BYTES = 4 * 1024 * 1024;
export const identity = (v) => typeof v === 'string' && /^[A-Za-z0-9_-]{1,256}$/.test(v);
export const fixtureName = (n) => `tl-verify-2026-11-22-${n}`;
export function stagingEnvironment(env) {
  check(env.NYXID_PROFILE === 'staging', 'staging_profile_required', true);
  return { ...Object.fromEntries(Object.entries(env).filter(([key]) => !key.startsWith('NYXID_'))),
    NYXID_PROFILE: 'staging' };
}
export function origin(value) {
  const url = new URL(value);
  check(url.protocol === 'https:' && url.origin === value && !url.username && !url.password,
    'exact_https_origin_required', true);
  return value;
}
export async function privateBytes(path, expectedHash) {
  check(isAbsolute(path) && path === await realpath(path), 'private_real_path_required', true);
  const outside = relative(ROOT, path);
  check(outside.startsWith('../') || isAbsolute(outside), 'private_input_in_repository', true);
  const stat = await lstat(path);
  check(stat.isFile() && stat.uid === process.getuid() && (stat.mode & 0o077) === 0
    && stat.size <= 8 * 1024 * 1024, 'private_input_permissions_or_size', true);
  const bytes = await readFile(path);
  if (expectedHash !== undefined) check(isHash(expectedHash) && hash(bytes) === expectedHash,
    'private_input_hash_mismatch', true);
  return bytes;
}
export async function outputDirectory(path) {
  check(isAbsolute(path) && resolve(path) === path, 'absolute_output_required', true);
  const parent = await realpath(resolve(path, '..'));
  const outside = relative(ROOT, parent);
  check(outside.startsWith('../') || isAbsolute(outside), 'evidence_in_repository', true);
}
export function validateConfig(value, env = process.env, now = Date.now()) {
  stagingEnvironment(env);
  const config = structuredClone(value);
  check(config.schema === 'cma-staging-driver-v1' && config.environment === 'staging'
    && config.profile === 'staging' && config.transport === 'browser-cookie', 'staging_config_required', true);
  origin(config.origin); origin(config.nyxidOrigin);
  check(identity(config.runId) && Date.parse(config.expiresAt) > now, 'authorization_expired', true);
  for (const key of ['candidateHash', 'serviceMapHash', 'policyHash', 'approvalHash', 'driverHash', 'reviewHash'])
    check(isHash(config[key]), 'authorization_hash_missing', true);
  check(config.reviewVerdict === 'GO' && config.policy?.toolsDisabled === true
    && config.policy?.networkDisabled === true && config.policy?.writesDisabled === true
    && isHash(config.policy.enforcementHash), 'enforced_workload_policy_required', true);
  check(Number.isSafeInteger(config.maxTurns) && config.maxTurns > 0 && config.maxTurns <= 780,
    'funded_turn_cap_required', true);
  check(Number.isFinite(config.maxTurnLiability) && config.maxTurnLiability > 0
    && Number.isFinite(config.modelCap) && config.modelCap >= config.maxTurnLiability
    && Number.isFinite(config.infrastructureCap) && config.infrastructureCap > 0, 'funding_required', true);
  check(Array.isArray(config.fixtures) && config.fixtures.length > 0 && config.fixtures.length <= 200,
    'authorized_fixtures_required', true);
  const indices = new Set(); const subjects = new Map();
  for (const f of config.fixtures) {
    check(Number.isSafeInteger(f.index) && f.index > 0 && !indices.has(f.index) && identity(f.subject),
      'fixture_identity_invalid', true);
    indices.add(f.index);
    check(typeof f.browserSample === 'boolean' && Number.isSafeInteger(f.createPerMinute)
      && f.createPerMinute >= 2 && Number.isSafeInteger(f.readPerMinute) && f.readPerMinute >= 10,
    'fixture_quota_required', true);
    for (const key of ['sandboxProfile', 'agentProfile']) {
      const p = f[key];
      check(identity(p?.id) && Number.isSafeInteger(p.revision) && p.revision > 0 && isHash(p.sha256),
        'authorized_profile_required', true);
    }
    check(isHash(f.storageStateHash) && typeof f.storageStatePath === 'string', 'session_authorization_required', true);
    const binding = JSON.stringify([f.createPerMinute, f.readPerMinute, f.storageStateHash, f.storageStatePath]);
    check(!subjects.has(f.subject) || subjects.get(f.subject) === binding, 'subject_quota_or_session_conflict', true);
    subjects.set(f.subject, binding);
  }
  check(config.fixtures.some((f) => f.browserSample), 'browser_observer_required', true);
  return config;
}
export function validateAuthority(config, receipt, { cleanup = false, now = Date.now() } = {}) {
  check(receipt?.schema === 'cma-staging-driver-authority-v1' && receipt.profile === 'staging' && receipt.environment === 'staging'
    && receipt.origin === config.origin && receipt.nyxidOrigin === config.nyxidOrigin
    && receipt.runId === config.runId && receipt.transport === config.transport, 'environment_binding_mismatch', true);
  for (const key of ['candidateHash', 'serviceMapHash', 'policyHash', 'approvalHash', 'driverHash', 'reviewHash', 'configHash'])
    check(receipt[key] === config[key], 'environment_drift', true);
  check(Number.isFinite(Date.parse(receipt.observedAt)) && now - Date.parse(receipt.observedAt) <= 30000
    && Date.parse(receipt.observedAt) <= now + 5000, 'authority_stale', true);
  check(Array.isArray(receipt.subjectHashes)
    && config.fixtures.every((f) => receipt.subjectHashes.includes(hash(f.subject))), 'subject_not_authorized', true);
  if (!cleanup) {
    check(Date.parse(config.expiresAt) > now && receipt.stop === false && receipt.healthReady === true
      && receipt.providerReady === true && receipt.browserReady === true, 'monitor_stop_or_unready', true);
    check(receipt.capacityHomes >= config.fixtures.length && receipt.infrastructureReserved <= config.infrastructureCap
      && Number.isFinite(receipt.infrastructureReserved) && receipt.infrastructureReserved >= 0,
    'capacity_or_infrastructure_budget', true);
    check(Number.isFinite(receipt.spentAndReserved) && receipt.spentAndReserved >= 0,
      'provider_accounting_missing', true);
  }
  return receipt;
}
