import {createHash} from 'node:crypto';
import {isDeepStrictEqual} from 'node:util';

export function requireValue(value, code) { if (!value) throw new Error(code); }
export const identity = value => typeof value === 'string' && /^[-a-zA-Z0-9_]{1,160}$/.test(value);
const digest = value => typeof value === 'string' && /^sha256:[a-f0-9]{64}$/.test(value);
const nonempty = value => typeof value === 'string' && value.length > 0 && value.length < 1024;
export const same = isDeepStrictEqual;

export function artifact(value) {
  requireValue(value && /^[a-f0-9]{40}$/.test(value.source ?? '') && digest(value.config)
    && typeof value.image === 'string'
    && /^(?:[a-z0-9][a-z0-9.:-]*\/)+[a-z0-9][a-z0-9._/-]*@sha256:[a-f0-9]{64}$/.test(value.image),
  'immutable_artifact_required');
  return {source:value.source, image:value.image, config:value.config};
}

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  }
  return value;
}
export const hash = value => createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex');

function metadata(value) {
  const m = value?.metadata;
  requireValue(m && identity(m.name) && nonempty(m.uid) && identity(m.namespace)
    && !m.deletionTimestamp, 'live_resource_identity_required');
  return {name:m.name, uid:m.uid, namespace:m.namespace};
}

export function readyContainer(pod, name, expected, selectedImage = expected.image) {
  const m = metadata(pod);
  requireValue(pod.status?.phase === 'Running'
    && pod.status?.conditions?.some(c => c.type === 'Ready' && c.status === 'True'),
  'pod_not_ready');
  const containers = pod.spec?.containers?.filter(c => c.name === name);
  const statuses = pod.status?.containerStatuses?.filter(c => c.name === name);
  requireValue(containers?.length === 1 && statuses?.length === 1, 'container_binding_missing');
  const c = containers[0], s = statuses[0];
  requireValue(c.image === selectedImage && s.ready === true && s.restartCount === 0
    && nonempty(s.state?.running?.startedAt) && Number.isFinite(Date.parse(s.state.running.startedAt))
    && /^containerd:\/\/[a-f0-9]{64}$/.test(s.containerID ?? ''), 'container_not_ready_or_replaced');
  // Kubernetes reports a manifest or config ID depending on the CRI. Accept
  // only those two immutable digests, never a tag or an unrelated image ID.
  const imageID = s.imageID?.replace(/^(?:docker-pullable|containerd):\/\//, '');
  requireValue([expected.config, expected.image, expected.image.split('@')[1]].includes(imageID),
    'runtime_image_id_mismatch');
  return {...m, container_id:s.containerID, image:expected.image, selected_image:selectedImage,
    image_id:s.imageID, started_at:s.state.running.startedAt, restarts:s.restartCount};
}

export function readerObservation(value, expected) {
  requireValue(same(artifact(value), expected), 'reader_artifact_mismatch');
  const selectedImage = value.local_image_alias ?? expected.image;
  requireValue(selectedImage === expected.image
    || /^cma-matrix-(old|new)-[a-f0-9]{12}(?::latest)?$/.test(selectedImage), 'reader_alias_invalid');
  // kind imports use a campaign-owned tag. Its observed image ID still must
  // equal the immutable manifest/config; the alias alone proves nothing.
  return {...expected, pod:readyContainer(value.pod, 'api', expected, selectedImage)};
}

export function lineage(detail, agentId) {
  requireValue(detail?.id === agentId && identity(agentId), 'agent_identity_mismatch');
  requireValue(detail.status === 'ready' && detail.sandbox?.status === 'active', 'agent_not_ready');
  const result = {agent_id:agentId, created_at:detail.created_at, desktop:detail.desktop};
  requireValue(nonempty(result.created_at) && Number.isFinite(Date.parse(result.created_at))
    && result.desktop === false, 'agent_creation_binding_invalid');
  for (const key of ['sandbox','workspace','worktree']) {
    requireValue(identity(detail[key]?.id), 'lineage_' + key + '_missing');
    result[key + '_id'] = detail[key].id;
  }
  requireValue(nonempty(detail.worktree.path), 'worktree_path_missing');
  result.worktree_path = detail.worktree.path;
  return result;
}

export function nativeObservation(value, retainedLineage, expected) {
  requireValue(value?.agent_id === retainedLineage.agent_id
    && value.sandbox_id === retainedLineage.sandbox_id, 'runtime_lineage_mismatch');
  const sandbox = metadata(value.sandbox);
  requireValue(value.sandbox.metadata.annotations?.['cma/sandbox'] === retainedLineage.sandbox_id,
    'native_sandbox_join_missing');
  const home = value.sandbox.metadata.annotations?.['cma/home-identity'];
  requireValue(nonempty(home), 'native_home_missing');
  const pod = readyContainer(value.pod, 'runtime', expected);
  requireValue(pod.namespace === sandbox.namespace
    && value.pod.metadata.ownerReferences?.some(o => o.uid === sandbox.uid && o.controller === true),
  'native_pod_owner_mismatch');
  const claims = (value.pod.spec.volumes ?? []).filter(v => v.persistentVolumeClaim)
    .map(v => v.persistentVolumeClaim.claimName);
  requireValue(claims.length > 0 && claims.length <= 8 && new Set(claims).size === claims.length
    && Array.isArray(value.pvcs) && value.pvcs.length === claims.length, 'native_pvc_set_invalid');
  const pvcs = value.pvcs.map(pvc => {
    const m = metadata(pvc);
    requireValue(m.namespace === sandbox.namespace && claims.includes(m.name)
      && pvc.status?.phase === 'Bound' && nonempty(pvc.spec?.volumeName), 'native_pvc_not_bound');
    return {...m, volume:pvc.spec.volumeName};
  }).sort((a,b) => a.name.localeCompare(b.name));
  requireValue(new Set(pvcs.map(p => p.name)).size === claims.length
    && new Set(pvcs.map(p => p.uid)).size === claims.length, 'native_pvc_duplicate');
  return {sandbox, home, pod, pvcs, artifact:expected};
}

export function completedResponse(value, agentId, responseId, marker) {
  requireValue(identity(responseId) && value?.id === responseId
    && value.conversation?.id === agentId, 'response_identity_mismatch');
  requireValue(value.status === 'completed' && Array.isArray(value.output)
    && value.output.some(item => item?.type === 'message' && item.role === 'assistant'
      && item.content?.some(part => part.type === 'output_text' && part.text?.includes(marker))),
  'completed_marker_response_missing');
  return structuredClone(value);
}

export function transcript(value, agentId) {
  requireValue(value?.agent_id === agentId && Number.isSafeInteger(value.through_seq)
    && value.through_seq >= 0 && Array.isArray(value.turns) && value.turns.length <= 64
    && !value.next, 'complete_transcript_required');
  let previous = 0;
  for (const turn of value.turns) {
    requireValue(Number.isSafeInteger(turn.turn_sequence) && turn.turn_sequence > previous
      && typeof turn.owner_message === 'string' && Array.isArray(turn.agent_messages)
      && turn.agent_messages.every(x => typeof x === 'string')
      && Array.isArray(turn.messages) && turn.outcome !== undefined, 'transcript_turn_invalid');
    previous = turn.turn_sequence;
  }
  return structuredClone(value);
}

export function preservedTranscript(before, after) {
  requireValue(after.through_seq >= before.through_seq && after.turns.length >= before.turns.length
    && before.turns.every((turn,i) => same(turn,after.turns[i])), 'retained_transcript_changed');
}

export function providerObservation(value, caseId, completed) {
  requireValue(value?.case_id === caseId && value.request_count === 1
    && value.completed_effect_count === completed && value.active_streams === (completed ? 0 : 1),
  'provider_effect_count_mismatch');
}
