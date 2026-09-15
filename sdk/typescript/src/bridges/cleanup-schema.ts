import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  record,
  tagged,
  text,
  type Output,
  type Parser,
} from '../schema.js';
import { agentTarget, boundedId, sameTarget } from './invoke-schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    if (Object.keys(record(value)).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected child cleanup field');
    return parse(value);
  };
}
function prefixed(prefix: string) {
  return (value: unknown): string => {
    const id = text(value);
    if (!id.startsWith(prefix) || !/^[0-9a-f]{64}$/.test(id.slice(prefix.length)))
      throw new ProtocolError('invalid child cleanup identity');
    return id;
  };
}
export const allocationId = prefixed('bral_');
export function cursor(value: unknown): string {
  const result = text(value);
  if (!result.length || new TextEncoder().encode(result).length > 1024 || /\p{Cc}/u.test(result))
    throw new ProtocolError('invalid opaque allocation cursor');
  return result;
}
function generation(value: unknown): number {
  const result = positiveInteger(value);
  if (result > 256) throw new ProtocolError('allocation generation exceeds 256');
  return result;
}
export const action = enumeration('remove_child');
export type BridgeChildCleanupAction = Output<typeof action>;
export const request = closed({
  expected_bridge_revision: positiveInteger,
  expected_allocation_revision: positiveInteger,
  action,
});
export type BridgeChildCleanupRequest = Output<typeof request>;
const manager = tagged(
  {
    human: closed({ kind: enumeration('human'), subject: boundedId }),
    api: closed({
      kind: enumeration('api'),
      subject: boundedId,
      delegation_id: nullable(enumeration()),
    }),
  },
  'kind',
);
const receipt = closed({
  id: prefixed('bclj_'),
  bridge_id: boundedId,
  allocation_id: allocationId,
  generation,
  target: agentTarget,
  request,
  requested_by: manager,
  accepted_at_ms: integer,
});
export type BridgeChildCleanupReceipt = Output<typeof receipt>;
export const progress = enumeration(
  'not_requested',
  'requested',
  'admission_closed',
  'removal_started',
  'process_closed',
  'home_removed',
  'worktree_removed',
  'complete',
);
export type BridgeChildCleanupProgress = Output<typeof progress>;
const holds = [
  'agent_busy',
  'workspace_busy',
  'pending_command',
  'uncertain_command',
  'pending_reply',
  'uncertain_reply',
  'other_workspace_children',
  'inventory_limit',
  'authority_unavailable',
  'parent_unavailable',
  'runtime_changed',
  'evidence_unavailable',
  'evidence_inconsistent',
] as const;
export const hold = enumeration(...holds);
export type BridgeChildCleanupHold = Output<typeof hold>;
const statusShape = closed({
  bridge_id: boundedId,
  allocation_id: allocationId,
  generation,
  allocation_revision: positiveInteger,
  target: agentTarget,
  receipt: nullable(receipt),
  progress,
  holds: array(hold),
  observed_at_ms: nullable(integer),
  agent_capacity_released: boolean,
  workspace_retained: boolean,
  history_retained: boolean,
});
export type BridgeChildCleanupStatus = Output<typeof statusShape>;
export function status(value: unknown): BridgeChildCleanupStatus {
  const result = statusShape(value);
  const accepted = result.receipt;
  if (
    !result.workspace_retained ||
    !result.history_retained ||
    result.agent_capacity_released !== (result.progress === 'complete') ||
    (result.progress === 'not_requested') !== (accepted === null) ||
    (accepted === null && (result.holds.length !== 0 || result.observed_at_ms !== null)) ||
    (accepted !== null && result.observed_at_ms === null) ||
    (result.progress === 'complete' && result.holds.length !== 0) ||
    result.holds.some(
      (value, index) =>
        index > 0 && holds.indexOf(value) <= holds.indexOf(result.holds[index - 1]!),
    ) ||
    (accepted !== null &&
      (accepted.bridge_id !== result.bridge_id ||
        accepted.allocation_id !== result.allocation_id ||
        accepted.generation !== result.generation ||
        !sameTarget(accepted.target, result.target) ||
        accepted.request.expected_allocation_revision !== result.allocation_revision))
  )
    throw new ProtocolError('child cleanup receipt, target or progress is inconsistent');
  return result;
}
export const placement = enumeration('existing_agent', 'new_sandbox', 'reuse_parent');
export type BridgeCleanupPlacement = Output<typeof placement>;
export const allocationState = enumeration(
  'reserved',
  'provisioning',
  'ready',
  'needs_connection',
  'failed_retryable',
  'failed_terminal',
  'retired',
);
export type BridgeCleanupAllocationState = Output<typeof allocationState>;
const allocation = closed({
  id: allocationId,
  bridge_id: boundedId,
  generation,
  revision: positiveInteger,
  target: agentTarget,
  placement,
  state: allocationState,
});
export type BridgeCleanupAllocation = Output<typeof allocation>;
export const page = closed({ items: array(allocation), next_cursor: nullable(cursor) });
export type BridgeCleanupAllocationPage = Output<typeof page>;
