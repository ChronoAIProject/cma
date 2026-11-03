import type { SandboxUsageView, SandboxUsageSummary } from '@chronoai/cma-sdk';
import type { OperatorClient } from '../operator/client';

export interface UsageFilters {
  readonly owner: string;
  readonly status: string;
  readonly sandbox: string;
}

export type UsageSortKey =
  | 'name'
  | 'sandbox_id'
  | 'owner'
  | 'status'
  | 'uptime_seconds'
  | 'cpu_millis_seconds'
  | 'memory_mib_seconds'
  | 'disk_gib_seconds'
  | 'tracking_started_at';
export interface UsageSort {
  readonly key: UsageSortKey;
  readonly direction: 'ascending' | 'descending';
}

/** Read the complete retained inventory before presenting globally sorted pages. */
export async function loadUsageInventory(client: OperatorClient['usage'], signal: AbortSignal) {
  const items: SandboxUsageView[] = [];
  const ids = new Set<string>();
  const cursors = new Set<string>();
  let cursor: string | undefined;
  // Same bounded inventory convention as the Operations owner picker. Never
  // silently truncate: incomplete results must not look globally sorted.
  for (let count = 0; count < 100; count += 1) {
    signal.throwIfAborted();
    const page = await client.adminSandboxes({ cursor, limit: 200, signal });
    signal.throwIfAborted();
    for (const row of page.items) {
      if (ids.has(row.sandbox_id)) throw new Error('Usage inventory repeated a Sandbox.');
      ids.add(row.sandbox_id);
      items.push(row);
    }
    if (page.next === null) return items;
    if (!page.next || cursors.has(page.next)) throw new Error('Usage inventory did not advance.');
    cursors.add(page.next);
    cursor = page.next;
  }
  throw new Error('Usage inventory exceeded its page limit.');
}

export function selectUsage(
  rows: readonly SandboxUsageView[],
  filters: UsageFilters,
  sort: UsageSort,
  ownerNames: ReadonlyMap<string, string>,
) {
  const search = filters.sandbox.trim().toLowerCase();
  const value = (row: SandboxUsageView): string | number => {
    if (sort.key === 'owner') return ownerNames.get(row.owner) ?? row.owner;
    if (sort.key in row.totals) return row.totals[sort.key as keyof typeof row.totals];
    return row[sort.key as 'name' | 'sandbox_id' | 'status' | 'tracking_started_at'];
  };
  return rows
    .filter(
      (row) =>
        (!filters.owner || row.owner === filters.owner) &&
        (!filters.status || row.status === filters.status) &&
        (!search || row.sandbox_id.toLowerCase().includes(search)),
    )
    .sort((left, right) => {
      const a = value(left);
      const b = value(right);
      const compared =
        typeof a === 'number' && typeof b === 'number' ? a - b : String(a).localeCompare(String(b));
      const stable = compared || left.sandbox_id.localeCompare(right.sandbox_id);
      return sort.direction === 'ascending' ? stable : -stable;
    });
}

/** Filter totals use the same recorded readings as the visible inventory. */
export function summarizeUsage(
  rows: readonly SandboxUsageView[],
  asOf: number,
): SandboxUsageSummary {
  const totals = {
    uptime_seconds: 0,
    cpu_millis_seconds: 0,
    memory_mib_seconds: 0,
    disk_gib_seconds: 0,
    cpu_core_hours: 0,
    memory_gib_hours: 0,
    disk_gib_hours: 0,
  };
  for (const row of rows) {
    totals.uptime_seconds += row.totals.uptime_seconds;
    totals.cpu_millis_seconds += row.totals.cpu_millis_seconds;
    totals.memory_mib_seconds += row.totals.memory_mib_seconds;
    totals.disk_gib_seconds += row.totals.disk_gib_seconds;
  }
  totals.cpu_core_hours = totals.cpu_millis_seconds / 3_600_000;
  totals.memory_gib_hours = totals.memory_mib_seconds / 3_686_400;
  totals.disk_gib_hours = totals.disk_gib_seconds / 3_600;
  return {
    as_of: asOf,
    measurement: 'allocated_resource_time',
    sandbox_count: rows.length,
    running_count: rows.filter((row) => row.allocation.running).length,
    incomplete_lifecycle_count: rows.filter((row) => !row.complete_lifecycle).length,
    tracking_started_at: rows.length
      ? Math.min(...rows.map((row) => row.tracking_started_at))
      : null,
    totals,
  };
}
