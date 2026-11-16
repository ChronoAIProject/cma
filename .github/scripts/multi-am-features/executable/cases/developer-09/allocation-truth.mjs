import {check} from './setup-common.mjs';
const zero = () => ({uptime_seconds: 0, cpu_millis_seconds: 0, memory_mib_seconds: 0, disk_gib_seconds: 0});
export function quantities(totals) {
  return {...totals, cpu_core_hours: totals.cpu_millis_seconds / 3600000,
    memory_gib_hours: totals.memory_mib_seconds / 3686400, disk_gib_hours: totals.disk_gib_seconds / 3600};
}
function accumulate(totals, allocation, seconds) {
  check(Number.isSafeInteger(seconds) && seconds >= 0, 'allocation_truth_duration');
  for (const [output, input] of [['cpu_millis_seconds', 'cpu_millis'], ['memory_mib_seconds', 'memory_mib'], ['disk_gib_seconds', 'disk_gib']]) {
    check(Number.isSafeInteger(allocation[input]) && allocation[input] >= 0, 'allocation_truth_capacity');
    totals[output] += allocation[input] * seconds;
    check(Number.isSafeInteger(totals[output]), 'allocation_truth_overflow');
  }
  if (allocation.running) totals.uptime_seconds += seconds;
}
export function intervalTotals(record, from, to) {
  check(Number.isSafeInteger(from) && Number.isSafeInteger(to) && from >= 0 && from <= to, 'allocation_truth_range');
  const totals = zero(); let end = record.tracking_started_at;
  const events = [...record.events].sort((a, b) => a.revision - b.revision);
  check(events.length > 0, 'allocation_truth_events_required');
  for (const event of events) {
    check(event.started_at === end && event.ended_at >= event.started_at, 'allocation_truth_interval_gap');
    accumulate(totals, event.allocation, Math.max(0, Math.min(to, event.ended_at) - Math.max(from, event.started_at)));
    end = event.ended_at;
  }
  const tail = events.at(-1).next_allocation;
  accumulate(totals, tail, Math.max(0, to - Math.max(from, end)));
  return quantities(totals);
}
export function cohortTotals(records, from, to, owner, sandboxId) {
  const selected = records.filter(r => (!owner || r.owner === owner) && (!sandboxId || r.sandbox_id === sandboxId));
  const totals = zero();
  for (const record of selected) {
    const value = intervalTotals(record, from, to);
    for (const name of Object.keys(totals)) {
      totals[name] += value[name];
      check(Number.isSafeInteger(totals[name]), 'allocation_truth_cohort_overflow');
    }
  }
  return quantities(totals);
}
export function expectedReport(records, query) {
  const filters = query.filters ?? {};
  check(!filters.runtime_version && !filters.sandbox_id && !filters.agent_id && !filters.search
    && !(filters.statuses?.length), 'basic_truth_filtered_variant_unavailable');
  const selected = records.filter(r => !filters.owner || r.owner === filters.owner);
  const totals = cohortTotals(selected, query.from, query.to);
  return {sandbox_count: selected.length, allocated_period: {
    cpu_core_hours: totals.cpu_core_hours, memory_gib_hours: totals.memory_gib_hours, disk_gib_hours: totals.disk_gib_hours}};
}
