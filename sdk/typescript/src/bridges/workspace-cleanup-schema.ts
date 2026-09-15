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
      throw new ProtocolError('unexpected Workspace cleanup field');
    return parse(value);
  };
}
function prefixed(prefix: string) {
  return (value: unknown): string => {
    const id = text(value);
    if (!id.startsWith(prefix) || !/^[0-9a-f]{64}$/.test(id.slice(prefix.length)))
      throw new ProtocolError('invalid Workspace cleanup identity');
    return id;
  };
}
const allocationId = prefixed('bral_');
function generation(value: unknown): number {
  const result = positiveInteger(value);
  if (result > 256) throw new ProtocolError('allocation generation exceeds 256');
  return result;
}
export const action = enumeration('remove_owned_workspace');
export type BridgeWorkspaceCleanupAction = Output<typeof action>;
export const request = closed({
  expected_bridge_revision: positiveInteger,
  expected_allocation_revision: positiveInteger,
  expected_workspace_revision: positiveInteger,
  child_cleanup_receipt_id: prefixed('bclj_'),
  action,
});
export type BridgeWorkspaceCleanupRequest = Output<typeof request>;
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
  id: prefixed('bwcl_'),
  bridge_id: boundedId,
  allocation_id: allocationId,
  generation,
  target: agentTarget,
  request,
  requested_by: manager,
  accepted_at_ms: integer,
});
export type BridgeWorkspaceCleanupReceipt = Output<typeof receipt>;
export const progress = enumeration(
  'not_requested',
  'requested',
  'admission_closed',
  'removal_started',
  'runtime_removed',
  'complete',
);
export type BridgeWorkspaceCleanupProgress = Output<typeof progress>;
const holds = [
  'allocation_not_retired',
  'workspace_not_owned',
  'child_cleanup_incomplete',
  'workspace_busy',
  'other_workspace_children',
  'inventory_limit',
  'authority_unavailable',
  'parent_unavailable',
  'runtime_unsupported',
  'runtime_changed',
  'evidence_unavailable',
  'evidence_inconsistent',
] as const;
export const hold = enumeration(...holds);
export type BridgeWorkspaceCleanupHold = Output<typeof hold>;
const statusShape = closed({
  bridge_id: boundedId,
  allocation_id: allocationId,
  generation,
  allocation_revision: positiveInteger,
  target: agentTarget,
  workspace_revision: nullable(positiveInteger),
  child_cleanup_receipt_id: nullable(prefixed('bclj_')),
  eligible: boolean,
  runtime_supported: boolean,
  receipt: nullable(receipt),
  progress,
  holds: array(hold),
  observed_at_ms: nullable(integer),
  workspace_capacity_released: boolean,
  parent_retained: boolean,
  history_retained: boolean,
});
export type BridgeWorkspaceCleanupStatus = Output<typeof statusShape>;
export function status(value: unknown): BridgeWorkspaceCleanupStatus {
  const result = statusShape(value);
  const accepted = result.receipt;
  if (
    !result.parent_retained ||
    !result.history_retained ||
    result.workspace_capacity_released !== (result.progress === 'complete') ||
    (result.progress === 'not_requested') !== (accepted === null) ||
    (result.eligible &&
      (!result.runtime_supported ||
        result.holds.length !== 0 ||
        accepted !== null ||
        result.workspace_revision === null ||
        result.child_cleanup_receipt_id === null)) ||
    (accepted !== null && (result.observed_at_ms === null || result.workspace_revision === null)) ||
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
        accepted.request.expected_allocation_revision !== result.allocation_revision ||
        accepted.request.child_cleanup_receipt_id !== result.child_cleanup_receipt_id))
  )
    throw new ProtocolError('Workspace cleanup receipt, target or progress is inconsistent');
  return result;
}
