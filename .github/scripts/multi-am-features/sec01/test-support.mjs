// Offline contract fixtures only; never runtime acceptance evidence.
import {CATEGORIES} from './cells.mjs';
export const HASH = 'a'.repeat(64);
export const SOURCE = 'b'.repeat(40);
export const IMAGE = '@sha256:' + 'c'.repeat(64);
export const OPTIONS = {source: SOURCE, image: IMAGE, runtimeSource: SOURCE, runtimeImage: IMAGE,
  lease: {scope: 'LOCAL_DISPOSABLE_TWO_AM', database_disposable: true,
    single_matrix_writer: true, restart_authority: 'PARENT_ONLY'}, record: async () => {}};
const member = am => ({instance_id: 'instance-' + am, pod_uid: 'pod-' + am,
  container_id: 'container-' + am, ready: true, image: IMAGE, source: SOURCE,
  origin: 'http://127.0.0.1:' + (am === 'a' ? '4100' : '4200')});
export function receipt(am) {
  const time = '2026-11-16T13:00:00.000Z';
  const snapshot = {...member(am), schema: 'c275-serving-v1', am, observed_at: time};
  return {am, serving: snapshot, serving_after: {...snapshot}, started_at: time, ended_at: time,
    request_id: 'request-' + am, attribution: {verdict: 'PASS', basis: 'pod_forward_before_after'}};
}
export const baseBinding = (cell, run_id) => ({
  schema: 'sec01-runtime-binding-v1', run_id, cell_id: cell.id, source_sha: SOURCE, image_digest: IMAGE,
  nonce: 'nonce-' + cell.id.toLowerCase(),
  state: cell.state, direction: cell.direction, runtime_id: 'runtime-' + cell.id.toLowerCase(),
  incarnation: 'incarnation-' + cell.id.toLowerCase(),
  runtime_source: SOURCE, runtime_image: IMAGE, volume_sha256: HASH, network_sha256: HASH,
  ams: {a: member('a'), b: member('b')},
  tenants: {a: {subject_sha256: HASH, organization_sha256: HASH},
    b: {subject_sha256: 'b'.repeat(64), organization_sha256: 'b'.repeat(64)}},
  tenant: {owner: cell.owner_tenant, foreign: cell.foreign_tenant}, evidence_sha256: HASH,
  lifecycle: {state: cell.state, evidence_sha256: HASH,
    own_data_before_sha256: HASH, own_data_after_sha256: HASH,
    transitions: (cell.state === 'cold' ? ['create'] : cell.state === 'warm' ? ['retire', 'sanitize', 'pool_claim']
      : ['pause', 'revoke', 'resume']).map((action, sequence) => ({action, sequence, run_id,
        runtime_id: 'runtime-' + cell.id.toLowerCase(), evidence_sha256: HASH})),
    new_runtime: cell.state === 'cold', tenant_history: cell.state !== 'cold',
    reused_after_retirement: cell.state === 'warm', sanitized: cell.state === 'warm',
    paused: cell.state === 'resumed', authorization_changed_while_paused: cell.state === 'resumed',
    restored_normally: cell.state === 'resumed'},
  serving: ['a', 'b'].map(receipt),
  cleanup: async () => {},
  targets: Object.fromEntries(CATEGORIES[cell.category].probes.map(probe => [probe,
    {tenant: cell.foreign_tenant, exists: true, run_id, evidence_sha256: HASH, denial_code: 'fixture_refused'}])),
  positives: Object.fromEntries(CATEGORIES[cell.category].positive.map(name => [name, {sha256: HASH, bytes: 4}])),
});

export function observation(spec, binding, context) {
  const base = {schema: 'sec01-observation-v1', id: spec.id, cell_id: spec.cell_id,
    state: spec.state, direction: spec.direction, run_id: context.run_id,
    runtime_id: binding.runtime_id, incarnation: binding.incarnation, am: context.am,
    nonce: context.nonce, serving_receipt: receipt(context.am),
    tenant: {owner: context.owner_tenant, foreign: context.foreign_tenant}, evidence_sha256: HASH};
  if (spec.expect === 'allow' && spec.kind === 'file') return {...base, kind: 'file', operations: [
    {operation: 'read', succeeded: true, bytes: 4, sha256: HASH, truncated: false},
    {operation: 'enumerate', succeeded: true}, {operation: 'write_open', succeeded: true}]};
  if (spec.expect === 'allow') return {...base, kind: spec.kind, status: 200,
    body_sha256: HASH, functional_operation: spec.operation, private_bytes: 0, private_fields: 0};
  const policy = {decision: 'deny', evidence_sha256: HASH, target_id: spec.id,
    nonce: context.nonce, runtime_id: context.runtime_id, run_id: context.run_id};
  if (spec.kind === 'tcp') return {...base, kind: 'tcp', requested_family: spec.required_family,
    addresses: [{family: spec.required_family === 'ipv6' ? 'ipv6' : 'ipv4',
      connected: false, errno: 13, failure_class: 'refused'}], policy};
  if (spec.kind === 'file') return {...base, kind: 'file', operations: [
    {operation: 'read', succeeded: false, errno: 13}, {operation: 'enumerate', succeeded: false, errno: 13},
    {operation: 'write_open', succeeded: false, errno: 13}], policy};
  return {...base, kind: spec.kind, status: 403, code: spec.denial_code,
    private_bytes: 0, private_fields: 0, redirect_authorization_count: 0,
    redirect_cookie_count: 0, old_token_reuse_count: 0, unauthorized_mint_count: 0,
    issuer_ledger_sha256: HASH, receiver_ledger_sha256: HASH, policy,
    ...(spec.scenario === 'revoked_credential' ? {revocation_receipt_sha256: HASH,
      revocation_sequence: 1, attempt_sequence: 2} : {})};
}

export function passingFixture() {
  const fixture = {
    evidence_kind: 'OFFLINE_SYNTHETIC',
    async prepareCell({cell, run_id}) { return baseBinding(cell, run_id); },
    async runtimeProbe({spec, binding, context}) { return observation(spec, binding, context); },
    async functionalProbe({spec, binding, context}) { return observation(spec, binding, context); },
    async credentialProbe({spec, binding, context}) { return observation(spec, binding, context); },
  };
  return fixture;
}
