import {contextFor} from './context.mjs';
import {need, digest, SHA256, verifyCapture} from './provenance.mjs';

// A lifecycle creation/terminal transition need not retain a live Agent lease.
// Its owner proof must come from the parent's captured worker/effect record,
// never from a later GET or the registrar. Missing correlation is BLOCKED.
export async function requireOwnerProof(ctx, state, code) {
  const {proofs, lifecycleTrust} = contextFor(ctx);
  need(Array.isArray(state.requestIds) && state.requestIds.length > 0, code + '_receipts_missing');
  const joined = [];
  for (const receipt of state.requestIds) {
    need(receipt.request_id && ['POST', 'PATCH', 'PUT', 'DELETE'].includes(receipt.method),
      code + '_mutation_receipt_missing');
    let proof;
    if (receipt.lifecycle) {
      const collector = ctx.fixture('lifecycle-evidence');
      need(typeof collector?.collect === 'function', 'lifecycle_owner_observer_missing');
      const envelope = await collector.collect({request_id: receipt.request_id, signal: ctx.signal});
      proof = verifyCapture(envelope, lifecycleTrust, 'mfunc01-lifecycle-owner-v1', receipt.request_id);
      need(proof.method === receipt.method && proof.path_sha256 === digest(receipt.path)
        && proof.status === receipt.status && proof.kind === 'lifecycle'
        && (!receipt.operation_id || proof.operation_sha256 === digest(receipt.operation_id)),
      code + '_lifecycle_join_mismatch');
      await ctx.observe(code + '_capture', async () => envelope);
    } else {
      const matches = proofs.filter(p => p.request_id === receipt.request_id);
      need(matches.length === 1, code + '_owner_proof_missing');
      [proof] = matches;
      need(proof.kind === 'non_owner' && proof.owner !== proof.ingress, code + '_nonowner_required');
    }
    need(proof.verdict === 'PASS' && proof.request_id === receipt.request_id
      && proof.agent_sha256 === digest(state.agentId) && ['a', 'b'].includes(proof.owner)
      && proof.ingress === receipt.am && Number.isSafeInteger(proof.owner_epoch) && proof.owner_epoch > 0
      && SHA256.test(proof.owner_instance_sha256 ?? '') && SHA256.test(proof.ingress_instance_sha256 ?? ''),
    code + '_invalid');
    joined.push(proof);
  }
  await ctx.observe(code, async () => ({proofs: joined}));
  return joined;
}
