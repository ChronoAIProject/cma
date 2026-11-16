const boundaries = new Map([
  ...['task.create', 'task.existing-location', 'task.repository', 'task.machine-create'].map(id => [id, 'create_to_usable']),
  ['agent.open', 'open_to_usable'], ['agent.send', 'send_to_acknowledgement'],
  ['agent.second-send', 'send_to_acknowledgement'], ['agent.interrupt', 'interrupt_to_terminal'],
  ['agent.interrupt-with-message', 'replacement_to_acknowledgement'], ['agent.stop', 'stop_to_terminal'],
  ...['1h', '24h', '7d', '30d', 'custom-filter', 'refresh'].map(id => ['usage.' + id, 'usage_to_complete_render']),
]);
export const latencyBoundary = row => boundaries.get(row.case_id.replace(/^C275:cma-user\./, ''));
export function latencyVerdict(row, result) {
  if (row.target_seconds == null) return 'NO_TARGET';
  const boundary = latencyBoundary(row);
  const samples = result.action_timings?.filter(sample => sample.boundary === boundary) ?? [];
  if (!boundary || !samples.length) return 'MISSING';
  return samples.every(sample => sample.status === 'PASS' && Number.isFinite(sample.elapsed_ms)
    && sample.elapsed_ms >= 0 && sample.elapsed_ms <= row.target_seconds * 1000) ? 'PASS' : 'FAIL';
}
