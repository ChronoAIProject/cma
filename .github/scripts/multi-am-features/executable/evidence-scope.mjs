import {validServingReceipt} from './serving.mjs';

export function validEgReceipt(receipt) {
  if (receipt?.scope !== 'eg' || !['a', 'b'].includes(receipt.replica) || receipt.am !== undefined) return false;
  const snapshots = [receipt.serving, receipt.serving_after];
  if (!snapshots.every(value => value?.schema === 'c275-eg-serving-v1' && value.replica === receipt.replica
    && value.scope === 'eg' && /^[a-f0-9]{40}$/.test(value.source ?? '')
    && /@sha256:[a-f0-9]{64}$/.test(value.image ?? ''))
    || snapshots[0].source !== snapshots[1].source || snapshots[0].image !== snapshots[1].image) return false;
  // Reuse process/window validation without classifying the recorded EG process
  // as an AM. The public result retains only the original EG-scoped receipt.
  const convert = value => ({...value, schema: 'c275-serving-v1', am: value.replica});
  return validServingReceipt({...receipt, am: receipt.replica,
    serving: convert(receipt.serving), serving_after: convert(receipt.serving_after)});
}

export const externalPlatform = id => ({'C275:EG-032': 'telegram', 'C275:EG-033': 'discord',
  'C275:EG-034': 'whatsapp'})[id];

export function validPlatformProof(proof, platform, event) {
  return proof?.schema === 'multi-am-external-platform-v1' && proof.kind === 'actual_platform'
    && proof.platform === platform && proof.synthetic === false
    && ['event_sha256', 'conversation_sha256', 'response_sha256'].every(key =>
      /^[a-f0-9]{64}$/.test(proof[key] ?? '') && proof[key] === event?.[key])
    && proof.delivery?.platform === platform && proof.delivery?.observed === true
    && /^[a-f0-9]{64}$/.test(proof.delivery?.artifact_sha256 ?? '')
    && proof.sender?.platform === platform && proof.sender?.authenticated === true
    && proof.sender?.independent === true
    && /^[a-f0-9]{64}$/.test(proof.sender?.artifact_sha256 ?? '')
    && proof.sender.artifact_sha256 !== proof.delivery.artifact_sha256;
}

export function platformComplete(id, result) {
  const platform = externalPlatform(id);
  if (!platform) return true;
  const events = result.platform_events ?? [], proofs = result.external_platform_proofs ?? [];
  return events.length > 0 && events.every(event => proofs.some(proof => validPlatformProof(proof, platform, event)))
    && result.requests?.length > 0 && result.requests.every(validServingReceipt)
    && result.owner_proofs?.some(proof => proof.verdict === 'PASS' && proof.owner !== proof.ingress);
}
