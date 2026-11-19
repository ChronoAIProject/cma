// The executable expansion of the five M-SEC-01 runtime categories.
// Tenant direction is deliberately part of the identity: AM-a/AM-b are
// ingress processes; tenant-a/tenant-b are independent security principals.
export const STATES = Object.freeze(['cold', 'warm', 'resumed']);
export const DIRECTIONS = Object.freeze([
  Object.freeze({id: 'a_to_b', owner: 'a', foreign: 'b'}),
  Object.freeze({id: 'b_to_a', owner: 'b', foreign: 'a'}),
]);

const category = (id, probes, positive) => Object.freeze({id, probes, positive});

export const CATEGORIES = Object.freeze({
  network: category('NET', [
    'peer_ipv4', 'peer_ipv6', 'peer_dns', 'private_service',
    'database', 'control_plane', 'redirect_destination', 'proxy_direct',
  ], ['dns_canary', 'nyxid_canary', 'model_canary', 'github_canary', 'object_store_canary']),
  file: category('FILE', [
    'foreign_home', 'foreign_workspace', 'foreign_volume', 'traversal',
    'symlink', 'proc_root_alias', 'retired_recycled', 'host_mount',
  ], ['owner_canary_read', 'owner_canary_hash']),
  metadata: category('META', [
    'link_local_metadata', 'metadata_dns', 'provider_ipv6_metadata', 'metadata_id',
  ], ['runtime_identity_attestation']),
  kubernetes: category('K8S', [
    'kubernetes_dns', 'service_ip', 'node_endpoint', 'control_plane_endpoint',
    'projected_token', 'host_mount_token',
  ], ['owner_service_api', 'synthetic_projected_identity']),
  credential: category('CRED', [
    'foreign_locator', 'wrong_audience', 'wrong_source', 'wrong_scope',
    'redirect_receiver', 'revoked_credential',
  ], ['owner_broker_access', 'owner_attachment_access']),
});

export const CELL_COUNT = Object.keys(CATEGORIES).length * STATES.length * DIRECTIONS.length;

export const cells = Object.freeze(Object.entries(CATEGORIES).flatMap(([name, spec]) =>
  STATES.flatMap(state => DIRECTIONS.map(direction => Object.freeze({
    id: `SEC-${spec.id}-${state.toUpperCase()}-${direction.owner.toUpperCase()}-TO-${direction.foreign.toUpperCase()}`,
    category: name,
    category_id: spec.id,
    state,
    direction: direction.id,
    owner_tenant: `tenant-${direction.owner}`,
    foreign_tenant: `tenant-${direction.foreign}`,
    probes: spec.probes,
    positive_controls: spec.positive,
  })))));

if (cells.length !== CELL_COUNT || new Set(cells.map(cell => cell.id)).size !== CELL_COUNT) {
  throw new Error('sec01_cell_expansion_invalid');
}

export function cellById(id) {
  const cell = cells.find(value => value.id === id);
  if (!cell) throw new Error('sec01_unknown_cell');
  return cell;
}
