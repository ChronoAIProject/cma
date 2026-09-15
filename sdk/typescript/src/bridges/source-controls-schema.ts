import { ProtocolError } from '../errors.js';
import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  record,
  text,
  type Parser,
  type Output,
} from '../schema.js';
import { actor, grantId, owner, pin, target, timestamp } from './invoke-schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    if (Object.keys(record(value)).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected source-control consent field');
    return parse(value);
  };
}
function prefixed(prefix: string) {
  return (value: unknown) => {
    const id = text(value);
    if (!new RegExp(`^${prefix}[0-9a-f]{64}$`).test(id))
      throw new ProtocolError('invalid source-control consent identity');
    return id;
  };
}
function bounded(max: number, minimum = 0) {
  return (value: unknown) => {
    const revision = integer(value);
    if (revision < minimum || revision > max)
      throw new ProtocolError('source-control consent revision is out of range');
    return revision;
  };
}
const action = enumeration('allow_sender_cancel_and_steer', 'revoke_sender_controls');
const requestShape = closed({
  expected_bridge_revision: positiveInteger,
  expected_grant_revision: bounded(2, 1),
  expected_policy_revision: bounded(255),
  action,
});
export type BridgeSourceControlRequest = Output<typeof requestShape>;
export type BridgeSourceControlAction = Output<typeof action>;
export function request(value: unknown): BridgeSourceControlRequest {
  const result = requestShape(value);
  if (result.action === 'allow_sender_cancel_and_steer' && result.expected_policy_revision === 255)
    throw new ProtocolError('the last consent revision is reserved for revocation');
  return result;
}
const policyShape = closed({
  id: prefixed('brscp_'),
  grant_id: grantId,
  owner,
  pin,
  target,
  revision: bounded(256, 1),
  enabled: boolean,
  scope: enumeration('source_sender'),
  approval_mode: enumeration('dashboard_only'),
  updated_at: timestamp,
  updated_by: actor,
});
export type BridgeSourceControlPolicy = Output<typeof policyShape>;
export type BridgeSourceControlScope = BridgeSourceControlPolicy['scope'];
export type BridgeSourceControlApprovalMode = BridgeSourceControlPolicy['approval_mode'];
export function policy(value: unknown): BridgeSourceControlPolicy {
  const result = policyShape(value);
  if (result.enabled && result.revision === 256)
    throw new ProtocolError('the terminal consent revision must remain revoked');
  return result;
}
export const state = closed({ policy: nullable(policy) });
export type BridgeSourceControlState = Output<typeof state>;
const receiptShape = closed({ id: prefixed('brscc_'), request, result: policy });
export type BridgeSourceControlReceipt = Output<typeof receiptShape>;
export function receipt(value: unknown): BridgeSourceControlReceipt {
  const receipt = receiptShape(value);
  if (
    receipt.result.revision !== receipt.request.expected_policy_revision + 1 ||
    receipt.result.enabled !== (receipt.request.action === 'allow_sender_cancel_and_steer') ||
    (receipt.result.enabled && receipt.request.expected_grant_revision !== 1)
  )
    throw new ProtocolError('source-control receipt does not match its admitted consent');
  return receipt;
}
export function sameRequest(a: BridgeSourceControlRequest, b: BridgeSourceControlRequest) {
  return (
    a.action === b.action &&
    a.expected_bridge_revision === b.expected_bridge_revision &&
    a.expected_grant_revision === b.expected_grant_revision &&
    a.expected_policy_revision === b.expected_policy_revision
  );
}
