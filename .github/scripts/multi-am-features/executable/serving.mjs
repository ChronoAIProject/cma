// Shared c275-serving-v1 contract from the frozen sole-parent successor.
// A label is a routing preference. Only a frozen Pod/process/origin is evidence.
const requireValue = (value, code) => {
  if (!value) throw Object.assign(new Error(code), {evidenceGap:true, attribution_failure:true});
};
export const SERVING_SCHEMA = 'c275-serving-v1';
export function servingBinding(am, topology) {
  const pod = topology?.ams?.[am];
  requireValue(pod?.ready === true && pod.draining !== true, 'serving_ready_pod_required');
  for (const field of ['pod_uid', 'instance_id', 'container_id', 'origin']) {
    requireValue(typeof pod[field] === 'string' && pod[field].trim(), 'serving_' + field + '_required');
  }
  const origin = new URL(pod.origin);
  requireValue(origin.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname)
    && !origin.username && !origin.password && origin.pathname === '/' && !origin.search && !origin.hash,
  'serving_pod_forward_origin_required');
  return Object.freeze({schema:SERVING_SCHEMA, am, origin:origin.origin, pod_uid:pod.pod_uid,
    instance_id:pod.instance_id, container_id:pod.container_id, ready:true,
    observed_at:new Date().toISOString()});
}
export function sameServing(expected, actual) {
  return expected?.schema === SERVING_SCHEMA && actual?.schema === SERVING_SCHEMA
    && ['am', 'origin', 'pod_uid', 'instance_id', 'container_id'].every(key =>
      typeof expected[key] === 'string' && expected[key].trim() && actual[key] === expected[key]);
}
export function requireServing(expected, actual) {
  requireValue(sameServing(expected, actual), 'serving_binding_changed');
}
export function requireServingReceipt(expected, response) {
  try {
    const timing = response?.timing ?? response;
    requireServing(expected, timing?.serving);
    requireServing(expected, timing?.serving_after);
    requireValue(timing?.attribution?.verdict === 'PASS' && timing.attribution.basis === 'pod_forward_before_after',
      'serving_attribution_unproved');
    requireValue(typeof timing.request_id === 'string' && timing.request_id.trim(), 'serving_request_id_required');
    const stamps = [timing.serving.observed_at,timing.started_at,timing.ended_at,timing.serving_after.observed_at].map(Date.parse);
    requireValue(stamps.every(Number.isFinite) && stamps.every((value,index)=>!index||value>=stamps[index-1]),
      'serving_observation_window_unproved');
    return timing;
  } catch(error) {if (response) error.response=response;throw error;}
}
export async function finishServing(topology, serving, requestId, response) {
  try {
    const after = servingBinding(serving.am, await topology());
    response.serving_after = after;
    requireServing(serving, after);
    requireValue(typeof requestId === 'string' && requestId.trim(), 'serving_request_id_required');
    response.attribution = {verdict:'PASS', basis:'pod_forward_before_after'};
  } catch (error) {
    response.attribution = {verdict:'UNPROVED', basis:'pod_forward_before_after', error:String(error.message)};
    throw Object.assign(error, {response, evidenceGap:true, attribution_failure:true});
  }
}

export function validServingReceipt(receipt) {
  try {
    if (!['a', 'b'].includes(receipt?.am) || receipt.serving?.am !== receipt.am) return false;
    for (const snapshot of [receipt.serving, receipt.serving_after]) {
      servingBinding(receipt.am, {ams: {[receipt.am]: snapshot}});
    }
    requireServingReceipt(receipt.serving, receipt);
    return true;
  } catch { return false; }
}
