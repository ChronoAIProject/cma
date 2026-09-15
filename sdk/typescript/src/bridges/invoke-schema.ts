import { ProtocolError } from '../errors.js';
import {
  array,
  enumeration,
  nullable,
  object,
  positiveInteger,
  record,
  tagged,
  text,
  type Parser,
  type Output,
} from '../schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    const source = record(value);
    if (Object.keys(source).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected bridge invoke field');
    return parse(value);
  };
}
export function boundedId(value: unknown): string {
  const result = text(value);
  if (!/^[A-Za-z0-9_\-.:/]{1,256}$/.test(result))
    throw new ProtocolError('invalid bounded bridge invoke identifier');
  return result;
}
export function grantId(value: unknown): string {
  const result = text(value);
  if (!/^brinv_[0-9a-f]{64}$/.test(result))
    throw new ProtocolError('invalid bridge invoke grant ID');
  return result;
}
function digest(value: unknown): string {
  const result = text(value);
  if (!/^[0-9a-f]{64}$/.test(result)) throw new ProtocolError('invalid bridge invoke digest');
  return result;
}
function timestamp(value: unknown): string {
  const result = text(value);
  if (result.length === 0 || result.length > 128 || /\p{Cc}/u.test(result))
    throw new ProtocolError('invalid bridge invoke timestamp');
  return result;
}
const owner = closed({ kind: enumeration('person', 'organization'), subject: boundedId });
const actor = tagged(
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
function sameActor(a: Output<typeof actor>, b: Output<typeof actor>): boolean {
  return a.kind === b.kind && a.subject === b.subject;
}
function configRevision(value: unknown): number {
  const revision = positiveInteger(value);
  if (revision > 256) throw new ProtocolError('bridge invoke configuration revision exceeds 256');
  return revision;
}
export const agentTarget = closed({
  sandbox_id: boundedId,
  workspace_id: boundedId,
  worktree_id: boundedId,
  agent_id: boundedId,
});
export type BridgeAgentTarget = Output<typeof agentTarget>;
export function sameTarget(a: BridgeAgentTarget, b: BridgeAgentTarget): boolean {
  return (
    a.sandbox_id === b.sandbox_id &&
    a.workspace_id === b.workspace_id &&
    a.worktree_id === b.worktree_id &&
    a.agent_id === b.agent_id
  );
}
const consent = enumeration('use_retained_agent_context');
const pin = closed({ bridge_id: boundedId, config_revision: configRevision });
const target = closed({
  ids: agentTarget,
  sandbox_created_at: timestamp,
  workspace_created_at: timestamp,
  agent_created_at: timestamp,
  agent_creation_fingerprint: digest,
  worktree_identity_digest: digest,
});
const state = tagged(
  {
    active: closed({ kind: enumeration('active') }),
    revoked: closed({ kind: enumeration('revoked'), at: timestamp, actor }),
  },
  'kind',
);
const grantShape = closed({
  id: grantId,
  revision: positiveInteger,
  owner,
  pin,
  target,
  consent: enumeration('use_retained_agent_context', 'use_channel_created_agent'),
  issued_at: timestamp,
  issued_by: actor,
  state,
});
export type BridgeInvokeGrant = Output<typeof grantShape>;
export function grant(value: unknown): BridgeInvokeGrant {
  const result = grantShape(value);
  if (result.revision !== (result.state.kind === 'active' ? 1 : 2))
    throw new ProtocolError('bridge invoke revision does not match its terminal lifecycle');
  return result;
}
const receiptShape = closed({
  id: digest,
  request_digest: digest,
  bridge_id: boundedId,
  actor,
  result: grant,
});
export type BridgeInvokeReceipt = Output<typeof receiptShape>;
export function receipt(value: unknown): BridgeInvokeReceipt {
  const result = receiptShape(value);
  const attributed =
    result.result.state.kind === 'active' ? result.result.issued_by : result.result.state.actor;
  if (result.bridge_id !== result.result.pin.bridge_id || !sameActor(result.actor, attributed))
    throw new ProtocolError('bridge invoke receipt does not match its bridge or actual actor');
  return result;
}
export const page = closed({ items: array(grant), next_cursor: nullable(grantId) });
const readinessShape = closed({
  bridge_id: boundedId,
  revision: positiveInteger,
  config_revision: configRevision,
  status: enumeration('not_applicable', 'consent_required', 'consent_present'),
  candidate: nullable(grant),
});
export type BridgeInvokeReadiness = Output<typeof readinessShape>;
export function readiness(value: unknown): BridgeInvokeReadiness {
  const result = readinessShape(value);
  if (
    result.config_revision > result.revision ||
    (result.status === 'consent_present') !== (result.candidate !== null)
  )
    throw new ProtocolError('inconsistent bridge invoke readiness');
  if (
    result.candidate !== null &&
    (result.candidate.state.kind !== 'active' ||
      result.candidate.pin.bridge_id !== result.bridge_id ||
      result.candidate.pin.config_revision !== result.config_revision)
  )
    throw new ProtocolError('bridge invoke candidate does not match the current configuration');
  return result;
}
export const issueRequest = closed({
  expected_bridge_revision: positiveInteger,
  target: agentTarget,
  shared_context: consent,
});
export const revokeRequest = closed({
  expected_bridge_revision: positiveInteger,
  expected_revision: positiveInteger,
});
export type IssueBridgeInvokeGrantRequest = Output<typeof issueRequest>;
export type RevokeBridgeInvokeGrantRequest = Output<typeof revokeRequest>;
export type BridgeInvokePage = Output<typeof page>;
export type BridgeInvokeGrantState = Output<typeof state>;
export type BridgeSharedContextConsent = Output<typeof consent>;
export type BridgeInvokeConsentStatus = BridgeInvokeReadiness['status'];
export type BridgeInvokePin = Output<typeof pin>;
export type BridgeInvokeTarget = Output<typeof target>;
