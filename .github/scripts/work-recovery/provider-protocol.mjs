// Synthetic key-provider wire contract for ordinary-work recovery fixtures.
import { createHash } from 'node:crypto';

export const fixtureIdentity = Object.freeze({
  owner: 'fixture-owner', deployment: 'work-recovery-fixture',
  service: 'fixture-service', slug: 'chrono-llm-public', node: 'fixture-node',
});

export function scopePlan(request) {
  const id = fixtureIdentity;
  if (!request || Object.keys(request).length !== 1
    || JSON.stringify(request.selected_service_ids) !== JSON.stringify([id.service])) {
    throw new Error('unsupported_scope_request');
  }
  const owner = { id: id.owner, type: 'personal' };
  const plan = {
    authority: 'nyxid', contract_version: '1', policy_version: 'api-key-scope-v1',
    authenticated_actor: owner, intended_key_owner: owner,
    services: [{ user_service_id: id.service, resource_owner: owner,
      node_grant: { type: 'required', node_ids: [id.node] } }],
    allowed_service_ids: [id.service], allowed_node_ids: [id.node],
    evaluated_at: new Date().toISOString(), key_purpose: 'general',
    freshness: { mode: 'mutation_revalidated_snapshot', precondition_field: 'scope_plan_digest',
      post_creation_drift: 'fail_closed' },
    completeness: { list_complete: true, no_duplicates: true,
      route_candidate_basis: 'active_configured_routes', transient_node_state_excluded: true },
  };
  // Match auth/keys/scope_plan.rs's length-prefixed v1 contract. This is a
  // synthetic authority response, never evidence of a real provider capability.
  const hash = createHash('sha256');
  const count = value => {
    const encoded = Buffer.alloc(8);
    encoded.writeBigUInt64BE(BigInt(value));
    hash.update(encoded);
  };
  const field = value => { count(Buffer.byteLength(value)); hash.update(value); };
  for (const value of ['nyxid-api-key-scope-plan-digest', plan.authority,
    plan.contract_version, plan.policy_version, id.owner, 'personal', id.owner,
    'personal', 'services']) field(value);
  count(1);
  for (const value of ['service', id.service, id.owner, 'required']) field(value);
  count(1); field(id.node);
  field('allowed_service_ids'); count(1); field(id.service);
  field('allowed_node_ids'); count(1); field(id.node);
  plan.normalized_grant_digest = `sha256:${hash.digest('hex')}`;
  return plan;
}

export function validateMint(body) {
  const id = fixtureIdentity;
  const fields = ['name', 'scopes', 'expires_at', 'allowed_service_ids', 'allowed_node_ids',
    'allow_all_services', 'allow_all_nodes', 'allow_auto_connected_services', 'scope_plan_digest'];
  const expected = scopePlan({ selected_service_ids: [id.service] });
  if (!body || typeof body !== 'object' || Array.isArray(body)
    || Object.keys(body).sort().join() !== fields.sort().join()
    || typeof body.name !== 'string'
    || !/^cma:work-recovery-fixture:mint_[a-f0-9]{32}$/.test(body.name)
    || body.scopes !== 'proxy read'
    || typeof body.expires_at !== 'string'
    || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/.test(body.expires_at)
    || !(Date.parse(body.expires_at) > Date.now())
    || JSON.stringify(body.allowed_service_ids) !== JSON.stringify([id.service])
    || JSON.stringify(body.allowed_node_ids) !== JSON.stringify([id.node])
    || body.allow_all_services !== false || body.allow_all_nodes !== false
    || body.allow_auto_connected_services !== false
    || body.scope_plan_digest !== expected.normalized_grant_digest) {
    throw new Error('unsupported_mint_request');
  }
  return body;
}
