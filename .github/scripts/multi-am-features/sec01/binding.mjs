import {sha} from './evaluate.mjs';

// Validate the retained receipt contract locally: this harness must not depend
// on a launcher or mistake a healthy response for process attribution.
export function validServingReceipt(receipt) {
  try {
    if (!['a', 'b'].includes(receipt?.am)) return false;
    const before = receipt.serving, after = receipt.serving_after;
    const fields = ['am', 'origin', 'pod_uid', 'instance_id', 'container_id'];
    for (const snapshot of [before, after]) {
      if (snapshot?.schema !== 'c275-serving-v1' || snapshot.am !== receipt.am
        || snapshot.ready !== true || snapshot.draining === true
        || !fields.every(key => typeof snapshot[key] === 'string' && snapshot[key].trim())) return false;
      const origin = new URL(snapshot.origin);
      if (origin.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname)
        || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash) return false;
    }
    if (!fields.every(key => before[key] === after[key])
      || receipt.attribution?.verdict !== 'PASS'
      || receipt.attribution.basis !== 'pod_forward_before_after'
      || typeof receipt.request_id !== 'string' || !receipt.request_id.trim()) return false;
    const stamps = [before.observed_at, receipt.started_at, receipt.ended_at, after.observed_at];
    if (!stamps.every(value => typeof value === 'string')) return false;
    const times = stamps.map(Date.parse);
    return times.every(Number.isFinite) && times.every((value, index) => !index || value >= times[index - 1]);
  } catch {
    return false;
  }
}

export const check = (value, code) => { if (!value) throw Error(code); };
export const identity = value => typeof value === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9_.:@/-]{0,255}$/.test(value);
export const sourceSha = value => typeof value === 'string' && /^[0-9a-f]{40}$/.test(value);
export const imageDigest = value => typeof value === 'string' && /^(?:[^\s@]+)?@sha256:[0-9a-f]{64}$/.test(value);
export const errorCode = error => /^[a-z][a-z0-9_]{0,100}$/.test(error?.message ?? '')
  ? error.message : 'fixture_operation_failed';

export function validateRuntimeBinding(cell, binding, runId, candidate) {
  check(binding?.schema === 'sec01-runtime-binding-v1', 'runtime_binding_schema');
  check(binding.run_id === runId && binding.cell_id === cell.id
    && binding.state === cell.state && binding.direction === cell.direction, 'runtime_binding_context');
  check(binding.source_sha === candidate.source && binding.image_digest === candidate.image,
    'candidate_binding_mismatch');
  check(identity(binding.runtime_id) && identity(binding.incarnation), 'runtime_identity_missing');
  check(sourceSha(binding.runtime_source) && imageDigest(binding.runtime_image)
    && binding.runtime_source === candidate.runtimeSource && binding.runtime_image === candidate.runtimeImage
    && sha(binding.volume_sha256) && sha(binding.network_sha256), 'runtime_image_volume_network_binding_missing');
  for (const am of ['a', 'b']) {
    const member = binding.ams?.[am];
    check(member?.ready === true && member.draining !== true
      && ['instance_id', 'pod_uid', 'container_id'].every(key => identity(member[key]))
      && member.source === candidate.source && member.image === candidate.image, 'two_am_binding_missing');
  }
  check(['instance_id', 'pod_uid', 'container_id'].every(key => binding.ams.a[key] !== binding.ams.b[key]),
    'two_am_processes_not_distinct');
  check(binding.tenant?.owner === cell.owner_tenant && binding.tenant?.foreign === cell.foreign_tenant,
    'tenant_binding_missing');
  const tenants = binding.tenants;
  check(tenants && ['a', 'b'].every(tenant => sha(tenants[tenant]?.subject_sha256)
    && sha(tenants[tenant]?.organization_sha256))
    && tenants.a.subject_sha256 !== tenants.b.subject_sha256
    && tenants.a.organization_sha256 !== tenants.b.organization_sha256, 'independent_tenant_identities_required');
  check(sha(binding.evidence_sha256), 'runtime_binding_evidence_missing');
  validateLifecycle(cell, binding);
  check(Array.isArray(binding.serving) && ['a', 'b'].every(am => binding.serving.some(receipt =>
    receipt.am === am && matchesServing(receipt, binding))), 'am_serving_not_proved');
  check(identity(binding.nonce) && binding.nonce.length >= 16, 'probe_nonce_missing');
}

function validateLifecycle(cell, binding) {
  const lifecycle = binding.lifecycle;
  check(lifecycle?.state === cell.state && sha(lifecycle.evidence_sha256), 'lifecycle_evidence_missing');
  if (cell.state === 'cold') check(lifecycle.new_runtime === true && lifecycle.tenant_history === false,
    'cold_runtime_not_new');
  if (cell.state === 'warm') check((lifecycle.reused_after_retirement === true && lifecycle.sanitized === true)
    || (lifecycle.pool_claimed === true && lifecycle.used_cross_tenant_reuse_refused === true),
  'warm_pool_transition_not_proved');
  if (cell.state === 'resumed') check(lifecycle.paused === true && lifecycle.authorization_changed_while_paused === true
    && lifecycle.restored_normally === true && sha(lifecycle.own_data_before_sha256)
    && lifecycle.own_data_before_sha256 === lifecycle.own_data_after_sha256,
  'resume_transition_not_proved');
  const steps = cell.state === 'cold' ? ['create'] : cell.state === 'resumed' ? ['pause', 'revoke', 'resume']
    : lifecycle.used_cross_tenant_reuse_refused === true ? ['retire', 'reuse_refused', 'pool_claim']
      : ['retire', 'sanitize', 'pool_claim'];
  const transitions = lifecycle.transitions;
  check(Array.isArray(transitions) && transitions.length === steps.length
    && transitions.every((row, i) => row.action === steps[i] && row.run_id === binding.run_id
      && row.runtime_id === binding.runtime_id && sha(row.evidence_sha256)
      && Number.isSafeInteger(row.sequence) && row.sequence >= 0
      && (i === 0 || transitions[i - 1].sequence < row.sequence)), 'lifecycle_order_unproved');
}

function matchesServing(receipt, binding) {
  const member = binding.ams[receipt?.am];
  return member && validServingReceipt(receipt)
    && ['instance_id', 'pod_uid', 'container_id', 'origin'].every(key => member[key] === receipt.serving[key]);
}

export function validateObservation(observation, spec, binding, context) {
  check(observation?.schema === 'sec01-observation-v1', 'observation_schema');
  check(observation.cell_id === spec.cell_id && observation.id === spec.id
    && observation.state === spec.state && observation.direction === spec.direction, 'observation_context');
  check(observation.run_id === context.run_id && observation.nonce === context.nonce
    && observation.runtime_id === binding.runtime_id && observation.incarnation === binding.incarnation
    && observation.am === context.am, 'observation_binding');
  check(sha(observation.evidence_sha256) && matchesServing(observation.serving_receipt, binding)
    && observation.serving_receipt.am === context.am, 'observation_serving_unproved');
  check(observation.tenant?.owner === context.owner_tenant
    && observation.tenant?.foreign === context.foreign_tenant, 'observation_tenant_binding');
  return observation;
}

// Raw output may contain third-party error bodies. Retain only the primitive's
// numeric/hash receipt fields. Never copy arbitrary provider output or headers.
const fields = new Set(('schema id cell_id state direction run_id runtime_id incarnation am nonce tenant owner foreign '
  + 'evidence_sha256 kind addresses family address_sha256 connected errno elapsed_ms resolution_errno '
  + 'operations operation succeeded entry_count bytes sha256 truncated invalid_canary_type status code '
  + 'private_bytes private_fields destination_connected redirect_authorization_count redirect_cookie_count '
  + 'old_token_reuse_count unauthorized_mint_count body_sha256 functional_operation issuer_ledger_sha256 '
  + 'receiver_ledger_sha256 revocation_receipt_sha256 revocation_sequence attempt_sequence policy decision target_id '
  + 'serving_receipt serving serving_after origin pod_uid instance_id container_id ready observed_at '
  + 'attribution verdict basis request_id started_at ended_at started_unix ended_unix').split(' '));
export function retainedObservation(value) {
  const raw = JSON.stringify(value);
  check(typeof raw === 'string' && raw.length <= 256 * 1024, 'observation_bound');
  const walk = input => {
    if (Array.isArray(input)) return input.map(walk);
    if (input && typeof input === 'object') return Object.fromEntries(Object.entries(input)
      .filter(([key]) => fields.has(key)).map(([key, child]) => [key, walk(child)]));
    return input;
  };
  return walk(value);
}
