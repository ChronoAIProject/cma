import { ProtocolError } from '../errors.js';
import {
  array,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  record,
  tagged,
  type Output,
  type Parser,
} from '../schema.js';
import { boundedId } from './invoke-schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    if (Object.keys(record(value)).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected Bridge usage field');
    return parse(value);
  };
}
const count = closed({ retained: integer, active: integer });
const resources = tagged(
  {
    available: closed({
      status: enumeration('available'),
      sandboxes: count,
      workspaces: count,
      worktrees: count,
      agents: count,
    }),
    redacted: closed({ status: enumeration('redacted') }),
    inventory_limited: closed({ status: enumeration('inventory_limited') }),
  },
  'status',
);
const value = closed({
  bridge_id: boundedId,
  bridge_revision: positiveInteger,
  observed_at_ms: integer,
  inventory_limit: positiveInteger,
  limits: closed({ max_partitions: positiveInteger, max_sandbox_reservations: positiveInteger }),
  allocations: nullable(
    closed({
      retained: integer,
      current: integer,
      ready: integer,
      retired: integer,
      sandbox_reservations: integer,
    }),
  ),
  target_resources: resources,
  capacity_reasons: array(
    enumeration(
      'partition_limit_reached',
      'sandbox_reservation_limit_reached',
      'retired_sandbox_reservations_retained',
      'shared_parent_policy_applies',
      'resource_access_required',
      'inventory_limit',
    ),
  ),
});
export type BridgeUsage = Output<typeof value>;
export type BridgeCapacityReason = BridgeUsage['capacity_reasons'][number];
export function usage(input: unknown): BridgeUsage {
  const result = value(input);
  const counts = result.allocations;
  const reasons = result.capacity_reasons;
  if (
    result.inventory_limit !== 256 ||
    result.limits.max_partitions > 100_000 ||
    result.limits.max_sandbox_reservations > result.limits.max_partitions ||
    new Set(reasons).size !== reasons.length ||
    (counts === null) !== (result.target_resources.status === 'inventory_limited') ||
    (counts === null) !== reasons.includes('inventory_limit') ||
    (result.target_resources.status === 'redacted') !== reasons.includes('resource_access_required')
  )
    throw new ProtocolError('inconsistent Bridge usage observation');
  if (
    counts === null &&
    reasons.some(
      (reason) => reason !== 'inventory_limit' && reason !== 'shared_parent_policy_applies',
    )
  )
    throw new ProtocolError('limited inventory cannot establish allocation capacity');
  if (counts !== null) {
    if (
      reasons.includes('retired_sandbox_reservations_retained') &&
      (counts.retired === 0 || counts.sandbox_reservations === 0)
    )
      throw new ProtocolError('retired reservation reason requires retained reservations');
    if (
      counts.retained > result.inventory_limit ||
      counts.current > counts.retained ||
      counts.ready + counts.retired > counts.retained ||
      counts.sandbox_reservations > counts.retained ||
      counts.current >= result.limits.max_partitions !==
        reasons.includes('partition_limit_reached') ||
      counts.sandbox_reservations >= result.limits.max_sandbox_reservations !==
        reasons.includes('sandbox_reservation_limit_reached')
    )
      throw new ProtocolError('inconsistent Bridge allocation counts');
    if (result.target_resources.status === 'available') {
      for (const count of [
        result.target_resources.sandboxes,
        result.target_resources.workspaces,
        result.target_resources.worktrees,
        result.target_resources.agents,
      ])
        if (count.active > count.retained || count.retained > counts.retained)
          throw new ProtocolError('inconsistent Bridge resource counts');
    }
  }
  return result;
}
