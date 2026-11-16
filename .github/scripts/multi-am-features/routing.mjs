import {createHash} from 'node:crypto';

const digest = value => createHash('sha256').update(value).digest('hex');
const requireValue = (value, message) => { if (!value) throw new Error(message); };

// The owner observation comes from the fixture's live lease/member join. A
// direct Pod forward identifies ingress independently of routing/relay logs.
export class NonOwnerRouter {
  constructor({adapter, record, maxObservationAgeMs = 2000, now = () => performance.now()}) {
    Object.assign(this, {adapter, record, maxObservationAgeMs, now});
  }

  async dispatch(agentId, operation) {
    return this.observe(agentId, operation);
  }

  async read(agentId, ingress, operation, method = 'GET') {
    requireValue(['a', 'b'].includes(ingress) && ['GET', 'HEAD'].includes(method), 'peer_read_method_required');
    return this.observe(agentId, operation, {ingress, method});
  }

  async at(agentId, ingress, operation, method) {
    requireValue(['a', 'b'].includes(ingress)
      && ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'].includes(method), 'explicit_ingress_required');
    return this.observe(agentId, operation, {ingress, method, kind: 'explicit_ingress'});
  }

  async observe(agentId, operation, read) {
    const observedAt = this.now();
    const owner = await this.adapter.ownerOf({agentId});
    requireValue(owner?.agent_id === agentId && ['a', 'b'].includes(owner.am)
      && Number.isSafeInteger(owner.epoch) && owner.epoch > 0
      && typeof owner.instance_id === 'string' && owner.instance_id.length > 0
      && typeof owner.pod_uid === 'string' && owner.pod_uid.length > 0,
    'live_owner_required');
    const topology = await this.adapter.topology();
    const ingress = read?.ingress ?? (owner.am === 'a' ? 'b' : 'a');
    const member = topology.ams?.[owner.am];
    const peer = topology.ams?.[ingress];
    const distinct = topology.ams?.[owner.am === 'a' ? 'b' : 'a'];
    requireValue(topology.shared_database === true && topology.coordination_enabled === true
      && member?.ready === true && peer?.ready === true
      && member.instance_id === owner.instance_id && member.pod_uid === owner.pod_uid
      && distinct?.ready === true && distinct.instance_id !== member.instance_id
      && distinct.pod_uid !== member.pod_uid,
    'owner_topology_mismatch');
    requireValue(this.now() - observedAt >= 0
      && this.now() - observedAt <= this.maxObservationAgeMs, 'owner_observation_expired');
    const proof = {agent_sha256: digest(agentId), owner: owner.am, ingress,
      kind: read ? (read.kind ?? 'peer_read') : 'non_owner', ...(read ? {method: read.method} : {}),
      owner_epoch: owner.epoch, owner_instance_sha256: digest(owner.instance_id),
      ingress_instance_sha256: digest(peer.instance_id), verdict: 'RUNNING'};
    let result;
    try {
      // Exactly one invocation: an uncertain mutation is never replayed.
      result = await operation(ingress, peer.origin);
      // Raw correlation IDs remain only in private journey evidence.
      proof.request_id = result?.timing?.request_id ?? null;
      requireValue(typeof proof.request_id === 'string' && proof.request_id.trim().length > 0,
        'response_request_id_required');
      const after = await this.adapter.ownerOf({agentId});
      requireValue(after?.instance_id === owner.instance_id && after.epoch === owner.epoch
        && after.pod_uid === owner.pod_uid && after.am === owner.am,
      'owner_changed_during_feature_request');
      proof.verdict = 'PASS';
      return result;
    } catch (error) {
      proof.verdict = 'FAIL';
      // An accepted mutation remains in original custody when attribution fails.
      // Keep only serializable receipt fields, never the live stream handle.
      if (result && error instanceof Error && error.response === undefined) {
        error.response = {status: result.status ?? result.timing?.status,
          body: result.body ?? null, timing: result.timing,
          ...(result.responseId ? {response_id: result.responseId, events: [...(result.events ?? [])]} : {})};
      }
      proof.request_id ??= error?.response?.timing?.request_id ?? null;
      throw error;
    } finally { await this.record(proof); }
  }
}
