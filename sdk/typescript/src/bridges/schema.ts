import { ProtocolError } from '../errors.js';
import { resourceActor, resourceOwner } from '../resource-access.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';

export const platform = enumeration('telegram', 'discord', 'lark', 'feishu');
export const partition = enumeration(
  'conversation_sender',
  'conversation',
  'thread_sender',
  'thread',
  'fixed_shared_agent',
  'sender_across_chats',
);
export const acceptedWork = enumeration('drain_original', 'suspend_original');
const profile = object({ id: text, revision: positiveInteger });
const target = object({ sandbox_id: text, workspace_id: text, worktree_id: text, agent_id: text });
export const placement = tagged(
  {
    existing_agent: object({ kind: enumeration('existing_agent'), target }),
    new_sandbox: object({
      kind: enumeration('new_sandbox'),
      sandbox_profile: profile,
      workspace_profile: profile,
      agent_profile: profile,
    }),
  },
  'kind',
);
const identityAdmission = tagged(
  {
    deny: object({ kind: enumeration('deny') }),
    open: object({ kind: enumeration('open') }),
    allowlist: object({ kind: enumeration('allowlist'), ids: array(text) }),
  },
  'kind',
);
export const admission = tagged(
  {
    all: object({ kind: enumeration('all') }),
    allowlist: object({ kind: enumeration('allowlist'), sender_ids: array(text) }),
    scoped: object({
      kind: enumeration('scoped'),
      policy: object({
        senders: identityAdmission,
        chats: identityAdmission,
        groups: enumeration('deny', 'all', 'mention', 'reply_to_bot', 'mention_or_reply_to_bot'),
      }),
    }),
  },
  'kind',
);
const configShape = object({
  display_name: text,
  binding: object({ bot_id: text, platform, route_ids: array(text), key_id: text }),
  partition,
  placement,
  admission,
  reply: enumeration('final_response', 'final_response_and_approvals', 'disabled'),
  limits: object({
    max_partitions: positiveInteger,
    max_sandboxes: positiveInteger,
    max_queued_messages: positiveInteger,
    max_messages_per_minute: positiveInteger,
    max_payload_bytes: positiveInteger,
    inbox_retention_seconds: positiveInteger,
    reply_secret_retention_seconds: positiveInteger,
  }),
});
export type BridgeConfiguration = Output<typeof configShape>;
export function configuration(value: unknown): BridgeConfiguration {
  const result = configShape(value);
  if (
    result.binding.route_ids.length === 0 ||
    result.binding.route_ids.length > 64 ||
    new Set(result.binding.route_ids).size !== result.binding.route_ids.length
  )
    throw new ProtocolError('bridge routes must be a bounded unique selection');
  const limits = result.limits;
  if (
    limits.max_sandboxes > limits.max_partitions ||
    limits.reply_secret_retention_seconds > limits.inbox_retention_seconds
  )
    throw new ProtocolError('bridge limits are inconsistent');
  return result;
}
function freshConfiguration(value: unknown): BridgeConfiguration {
  const result = configuration(value);
  if ((result.placement.kind === 'existing_agent') !== (result.partition === 'fixed_shared_agent'))
    throw new ProtocolError('fixed Agent placement requires explicit shared context');
  return result;
}
export const createRequest = object({ owner: resourceOwner, config: freshConfiguration });
export const updateRequest = object({
  expected_revision: positiveInteger,
  config: freshConfiguration,
  previous_work: nullable(acceptedWork),
});
export const emergency = object({
  ingress_stopped: boolean,
  execution_stopped: boolean,
  outbound_stopped: boolean,
});
export const control = tagged(
  {
    enable: object({ kind: enumeration('enable') }),
    disable: object({ kind: enumeration('disable'), accepted_work: acceptedWork }),
    retire: object({ kind: enumeration('retire'), accepted_work: acceptedWork }),
    emergency: object({ kind: enumeration('emergency'), controls: emergency }),
  },
  'kind',
);
export const controlRequest = object({ expected_revision: positiveInteger, command: control });
const state = tagged(
  {
    enabled: object({ kind: enumeration('enabled') }),
    disabled: object({ kind: enumeration('disabled'), accepted_work: acceptedWork }),
    retired: object({ kind: enumeration('retired'), accepted_work: acceptedWork }),
  },
  'kind',
);
const viewShape = object({
  id: text,
  owner: resourceOwner,
  display_name: text,
  revision: positiveInteger,
  config_revision: positiveInteger,
  state,
  emergency,
  bot_id: text,
  platform,
  can_manage: boolean,
  created_at: text,
  updated_at: text,
});
export type Bridge = Output<typeof viewShape>;
export function bridge(value: unknown): Bridge {
  const result = viewShape(value);
  if (result.config_revision > result.revision)
    throw new ProtocolError('bridge configuration revision exceeds its record revision');
  return result;
}
export const page = object({ items: array(bridge), next_cursor: optional(text) });
const receiptShape = object({
  bridge_id: text,
  owner: resourceOwner,
  actor: resourceActor,
  revision: positiveInteger,
  config_revision: positiveInteger,
  command: text,
  applied_at: text,
});
export type BridgeReceipt = Output<typeof receiptShape>;
export function receipt(value: unknown): BridgeReceipt {
  const result = receiptShape(value);
  if (result.config_revision > result.revision || !['human', 'api'].includes(result.actor.kind))
    throw new ProtocolError('bridge receipt has invalid management attribution or revisions');
  return result;
}
const credentialShape = object({ revision: integer, present: boolean, revoked: boolean });
export type BridgeCredentialStatus = Output<typeof credentialShape>;
export function credentialStatus(value: unknown): BridgeCredentialStatus {
  const result = credentialShape(value);
  if (result.present !== result.revision > 0 || (result.revoked && !result.present))
    throw new ProtocolError('bridge credential status is inconsistent');
  return result;
}
export const detail = object({
  bridge,
  config: configuration,
  callback_url: text,
  credential: credentialStatus,
});
export const validation = object({
  bridge_id: text,
  revision: positiveInteger,
  config_revision: positiveInteger,
  valid: boolean,
});
export const choices = object({
  bots: array(object({ id: text, platform, active: boolean })),
  keys: array(object({ id: text, active: boolean, callback_configured: boolean })),
  routes: array(
    object({ id: text, bot_id: nullable(text), key_id: text, platform: text, active: boolean }),
  ),
});
export type BridgePlatform = Output<typeof platform>;
export type BridgePartitionPolicy = Output<typeof partition>;
export type BridgePlacement = Output<typeof placement>;
export type BridgeAdmission = Output<typeof admission>;
export type BridgeAcceptedWork = Output<typeof acceptedWork>;
export type BridgeControl = Output<typeof control>;
export type CreateBridgeRequest = Output<typeof createRequest>;
export type UpdateBridgeRequest = Output<typeof updateRequest>;
export type ControlBridgeRequest = Output<typeof controlRequest>;
export type BridgeConfigurationDetail = Output<typeof detail>;
export type BridgeValidation = Output<typeof validation>;
export type BridgeConnectorChoices = Output<typeof choices>;
export type BridgePage = Output<typeof page>;
