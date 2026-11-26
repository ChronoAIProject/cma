import {createHash} from 'node:crypto';

const digest = value => createHash('sha256').update(value).digest('hex');
const requireValue = (value, message) => { if (!value) throw new Error(message); };
const evidencedOperations = new Map([
  ['PATCH /api/v1/agents/{id}/preferences', 'preferences'],
  ['PATCH /api/v1/agents/{id}/settings', 'settings'],
  ['POST /api/v1/agents/{id}/skills', 'skill_install'],
  ['POST /api/v2/agents/{id}/responses/{id}/controls', 'response_control'],
]);

// The owner observation comes from the fixture's live lease/member join. A
// direct Pod forward identifies ingress independently of routing/relay logs.
export class NonOwnerRouter {
  constructor({adapter, record, maxObservationAgeMs = 2000, now = () => performance.now()}) {
    Object.assign(this, {adapter, record, maxObservationAgeMs, now});
  }

  async dispatch(agentId, operation) {
    const observedAt = this.now();
    const owner = await this.adapter.ownerOf({agentId});
    requireValue(owner?.agent_id === agentId && ['a', 'b'].includes(owner.am)
      && Number.isSafeInteger(owner.epoch) && owner.epoch > 0
      && typeof owner.instance_id === 'string' && owner.instance_id.length > 0
      && typeof owner.pod_uid === 'string' && owner.pod_uid.length > 0,
    'live_owner_required');
    const topology = await this.adapter.topology();
    const ingress = owner.am === 'a' ? 'b' : 'a';
    const member = topology.ams?.[owner.am];
    const peer = topology.ams?.[ingress];
    requireValue(topology.shared_database === true && topology.coordination_enabled === true
      && member?.ready === true && peer?.ready === true
      && member.instance_id === owner.instance_id && member.pod_uid === owner.pod_uid
      && peer.instance_id !== member.instance_id && peer.pod_uid !== member.pod_uid,
    'owner_topology_mismatch');
    requireValue(this.now() - observedAt >= 0
      && this.now() - observedAt <= this.maxObservationAgeMs, 'owner_observation_expired');
    const proof = {agent_sha256: digest(agentId), owner: owner.am, ingress,
      owner_epoch: owner.epoch, owner_instance_sha256: digest(owner.instance_id),
      ingress_instance_sha256: digest(peer.instance_id), verdict: 'RUNNING'};
    let result;
    try {
      // Exactly one invocation: an uncertain mutation is never replayed.
      result = await operation(ingress);
      // Raw correlation IDs remain only in private journey evidence.
      proof.request_id = result?.timing?.request_id ?? null;
      requireValue(typeof proof.request_id === 'string' && proof.request_id.trim().length > 0,
        'response_request_id_required');
      const after = await this.adapter.ownerOf({agentId});
      requireValue(after?.instance_id === owner.instance_id && after.epoch === owner.epoch
        && after.pod_uid === owner.pod_uid && after.am === owner.am,
      'owner_changed_during_feature_request');
      const writer = evidencedOperations.get(`${result?.timing?.method} ${result?.timing?.route}`);
      if (writer && [200, 202].includes(result.status)) {
        const evidence = await this.adapter.peerWriteEvidence({am: owner.am,
          requestHash: digest(proof.request_id), epoch: owner.epoch, operation: writer});
        requireValue(evidence.matches === 1, 'verified_owner_write_missing_or_duplicated');
        proof.verified_owner_write = true;
      }
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
