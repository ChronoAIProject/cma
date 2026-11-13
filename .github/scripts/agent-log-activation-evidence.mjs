// Offline packet qualification only. No I/O, activation or authorization entrypoint.
export const operationLimitsMs = Object.freeze({
  agent_linux_ready: 5000,
  agent_desktop_ready: 5000,
  tab_open: 1000,
  send_complete: 1000,
  interrupt_complete: 1000,
  admin_sandbox_usage: 2000,
});

const bindingFields = ['source', 'image', 'generation', 'configuration', 'cohort', 'window'];
const finiteTime = value => Number.isFinite(value) && value >= 0;

// Quantiles and ACK times cannot replace client-send through final completion.
// Preserve every sample in the aggregate, including failures and cancellations.
export function qualifyOperations(evidence) {
  const reasons = [];
  const operations = {};
  if (!bindingFields.every(key => typeof evidence?.binding?.[key] === 'string'
      && evidence.binding[key].trim())) reasons.push('missing_attribution');
  for (const [name, limit] of Object.entries(operationLimitsMs)) {
    const samples = evidence?.operations?.[name];
    if (!Array.isArray(samples) || !samples.length) {
      reasons.push(`${name}:missing_samples`);
      continue;
    }
    const counts = { samples: samples.length, success: 0, error: 0, cancelled: 0,
      incomplete: 0, first_use: 0, maximum_ms: null, exceedances: [] };
    for (const [index, sample] of samples.entries()) {
      if (!sample || !['success', 'error', 'cancelled'].includes(sample.outcome)) {
        counts.incomplete++;
        reasons.push(`${name}:invalid_outcome`);
        continue;
      }
      counts[sample.outcome]++;
      if (sample.first_use === true) counts.first_use++;
      if (sample.outcome !== 'success') reasons.push(`${name}:unsuccessful`);
      if (!finiteTime(sample.client_send_ms) || !finiteTime(sample.completed_ms)
          || sample.completed_ms < sample.client_send_ms
          || sample.boundary !== 'whole_operation' || typeof sample.first_use !== 'boolean') {
        counts.incomplete++;
        reasons.push(`${name}:incomplete_boundary`);
        continue;
      }
      const elapsed = sample.completed_ms - sample.client_send_ms;
      counts.maximum_ms = Math.max(counts.maximum_ms ?? 0, elapsed);
      if (elapsed > limit) {
        counts.exceedances.push({ index, elapsed_ms: elapsed });
        reasons.push(`${name}:hard_limit`);
      }
    }
    if (!counts.first_use) reasons.push(`${name}:missing_first_use`);
    operations[name] = counts;
  }
  return { status: reasons.length ? 'HOLD' : 'QUALIFIED', reasons, operations };
}

export function readerDisposition(stage, packet) {
  const reasons = [];
  if (!['title', 'approval'].includes(stage)) reasons.push('unknown_reader_stage');
  if (!packet?.reviewed_scope_binding) reasons.push('reviewed_scope_binding_required');
  if (!packet?.individual_owner_approval) reasons.push('individual_owner_approval_required');
  if (packet?.fleet_percent !== 100 || !(packet?.stable_seconds >= 3600)) {
    reasons.push('fleet_100_plus_stable_hour_required');
  }
  if (packet?.mandatory_v2_reader_floor !== true) reasons.push('mandatory_v2_reader_floor_required');
  // #1901 deliberately implements no scope fence. Caller assertions cannot
  // release this HOLD; a future implementation needs its own reviewed change.
  reasons.push('legacy_v2_scope_fence_deferred');
  return { status: 'HOLD', reasons, patch: null };
}

const selector = 'CHRONO_AGENT_LOG_ADMISSION_AGENTS';
const percent = 'CHRONO_AGENT_LOG_FLEET_PERCENT';
const independent = ['CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED',
  'CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED'];

export function previewFleetPatch(before, patch) {
  const data = patch?.data;
  if (!data || Object.keys(patch).length !== 1 || Object.keys(data).length !== 2
      || data[selector] !== '' || !['0', '10', '50', '100'].includes(data[percent])) {
    return { status: 'HOLD', reason: 'invalid_selector_patch', patch: null };
  }
  if (data[percent] !== '0' && !independent.every(key => before?.[key] === 'false')) {
    return { status: 'HOLD', reason: 'initial_independent_controls_must_be_false', patch: null };
  }
  return { status: 'PREVIEW_ONLY', patch: { data: { ...data } }, after: { ...before, ...data } };
}
