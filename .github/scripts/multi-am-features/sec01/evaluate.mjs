import {createHash} from 'node:crypto';
export const digest = value => createHash('sha256').update(typeof value === 'string' ? value : JSON.stringify(value)).digest('hex');
export const sha = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const result = (status, reason) => ({status, reason});
const blocked = reason => result('BLOCKED', reason);
const fail = reason => result('FAIL', reason);
const pass = () => result('PASS', 'observed_isolation');
const finiteCount = value => Number.isSafeInteger(value) && value >= 0;

// A denial requires an independently retained, correlated policy record. An
// errno, timeout, app-level 403, missing target or boolean is insufficient.
function policyDenied(spec, observation, context) {
  const policy = observation?.policy;
  return policy?.decision === 'deny' && sha(policy.evidence_sha256)
    && policy.target_id === spec.id && policy.nonce === context.nonce
    && policy.runtime_id === context.runtime_id && policy.run_id === context.run_id;
}
function exists(spec, context) {
  return spec.owner_control?.tenant === context.foreign_tenant
    && spec.owner_control?.exists === true && sha(spec.owner_control?.evidence_sha256)
    && spec.owner_control?.run_id === context.run_id;
}

export function evaluateProbe(spec, observation, context) {
  if (!observation || observation.id !== spec.id || observation.kind !== spec.kind) return blocked('probe_missing_or_mismatched');
  // Never discard a demonstrated escape because another prerequisite failed.
  if (spec.expect === 'deny' && observation.kind === 'tcp'
    && Array.isArray(observation.addresses) && observation.addresses.some(row => row?.connected === true)) return fail('prohibited_tcp_connected');
  if (spec.expect === 'deny' && observation.kind === 'file'
    && Array.isArray(observation.operations) && observation.operations.some(row => row?.succeeded === true)) return fail('prohibited_file_operation_succeeded');
  if (observation.kind === 'credential' && (observation.private_bytes > 0
    || observation.private_fields > 0 || observation.redirect_authorization_count > 0
    || observation.redirect_cookie_count > 0 || observation.old_token_reuse_count > 0
    || observation.unauthorized_mint_count > 0)) return fail('credential_disclosure_or_effect');
  if (observation.kind === 'credential' && spec.expect === 'deny'
    && observation.status >= 200 && observation.status < 300) return fail('prohibited_credential_succeeded');
  if (observation.kind === 'http' && spec.expect === 'deny'
    && (observation.private_bytes > 0 || observation.private_fields > 0
      || observation.destination_connected === true)) return fail('prohibited_http_disclosure_or_connection');
  if (observation.kind === 'http' && spec.expect === 'deny'
    && observation.status >= 200 && observation.status < 300) return fail('prohibited_http_succeeded');
  if (observation.kind === 'file') {
    const operations = observation.operations;
    if (!Array.isArray(operations) || operations.length !== 3 || operations.some(x => !x || typeof x !== 'object')
      || new Set(operations.map(x => x.operation)).size !== 3
      || !['read', 'enumerate', 'write_open'].every(op => operations.some(x => x.operation === op))) return blocked('file_operations_incomplete');
    if (spec.expect === 'allow') {
      const read = operations.find(x => x.operation === 'read');
      return read.succeeded === true && read.truncated === false && sha(spec.expected_sha256)
        && read.sha256 === spec.expected_sha256 && read.bytes === spec.expected_bytes
        ? pass() : blocked('owner_canary_positive_failed');
    }
    if (!exists(spec, context)) return blocked('foreign_canary_existence_unproved');
    // Missing paths prove isolation only when the policy evidence attributes
    // their absence to the isolated namespace, with the owner canary present.
    return operations.every(x => x.succeeded === false && Number.isSafeInteger(x.errno) && x.errno > 0
      && (x.failure_class === 'permission_denied'
        || (x.failure_class === 'not_found' && observation.policy?.basis === 'isolated_namespace')))
      && policyDenied(spec, observation, context) ? pass() : blocked('file_denial_unattributed');
  }
  if (observation.kind === 'tcp') {
    if (spec.expect !== 'deny') return blocked('tcp_is_not_functional_positive');
    if (!exists(spec, context)) return blocked('target_liveness_unproved');
    if (observation.failure_class !== undefined) return blocked('tcp_primitive_unavailable');
    if (!Array.isArray(observation.addresses) || observation.addresses.length === 0) return blocked('dns_or_ip_family_unavailable');
    if (!['ipv4', 'ipv6', 'dns'].includes(spec.required_family)
      || observation.requested_family !== spec.required_family
      || !observation.addresses.every(row => ['ipv4', 'ipv6'].includes(row?.family)
        && (spec.required_family === 'dns' || row.family === spec.required_family))) return blocked('address_family_unproved');
    if (observation.truncated === true || !observation.addresses.every(x => x?.connected === false && Number.isInteger(x.errno))) return blocked('tcp_observation_incomplete');
    // Numeric errno values differ across platforms. Only a classified refusal
    // can join policy evidence; timeouts and missing protocol support cannot.
    if (!observation.addresses.every(row => row.failure_class === 'refused')) return blocked('tcp_timeout_or_unavailable');
    return policyDenied(spec, observation, context) ? pass() : blocked('tcp_denial_unattributed');
  }
  if (!['http', 'credential'].includes(observation.kind)) return blocked('unsupported_observation');
  if (!Number.isInteger(observation.status) || observation.status === 429
    || observation.status >= 500 || observation.status < 100) return blocked('dependency_unavailable');
  if (spec.expect === 'allow') {
    return observation.status === 200 && sha(spec.expected_sha256)
      && observation.body_sha256 === spec.expected_sha256 && observation.functional_operation === spec.operation
      ? pass() : blocked('functional_positive_failed');
  }
  if (!exists(spec, context)) return blocked('foreign_canary_existence_unproved');
  if (!finiteCount(observation.private_bytes) || !finiteCount(observation.private_fields)) return blocked('private_payload_counters_missing');
  if (observation.kind === 'credential') {
    const counters = ['redirect_authorization_count', 'redirect_cookie_count',
      'old_token_reuse_count', 'unauthorized_mint_count'];
    if (!counters.every(key => finiteCount(observation[key])) || !sha(observation.issuer_ledger_sha256)
      || !sha(observation.receiver_ledger_sha256)) return blocked('credential_ledger_incomplete');
    if (spec.scenario === 'revoked_credential' && (!sha(observation.revocation_receipt_sha256)
      || observation.revocation_sequence >= observation.attempt_sequence
      || !Number.isSafeInteger(observation.revocation_sequence)
      || !Number.isSafeInteger(observation.attempt_sequence))) return blocked('revocation_order_unproved');
  }
  if (![401, 403, 404].includes(observation.status) || observation.code !== spec.denial_code
    || !spec.denial_code) return blocked('typed_denial_missing');
  if (!policyDenied(spec, observation, context)) return blocked('denial_unattributed');
  return pass();
}

export function aggregate(statuses) {
  if (statuses.includes('FAIL')) return 'FAIL';
  if (!statuses.length || statuses.some(status => status !== 'PASS')) return 'BLOCKED';
  return 'PASS';
}
