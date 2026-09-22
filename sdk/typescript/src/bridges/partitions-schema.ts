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
import { allocationId, cursor } from './cleanup-schema.js';
import { boundedId } from './invoke-schema.js';

function closed<const Fields extends Readonly<Record<string, Parser<unknown>>>>(fields: Fields) {
  const parse = object(fields);
  return (value: unknown) => {
    if (Object.keys(record(value)).some((field) => !Object.hasOwn(fields, field)))
      throw new ProtocolError('unexpected partition status field');
    return parse(value);
  };
}
function generation(value: unknown) {
  const result = positiveInteger(value);
  if (result > 256) throw new ProtocolError('partition generation exceeds 256');
  return result;
}
function label(value: unknown) {
  const result = text(value);
  if (!result.trim() || [...result].length > 256 || /\p{Cc}/u.test(result))
    throw new ProtocolError('invalid sender display name');
  return result;
}
const ids = closed({
  sandbox_id: boundedId,
  workspace_id: boundedId,
  worktree_id: boundedId,
  agent_id: boundedId,
});
const resources = ['sandbox', 'workspace', 'worktree', 'agent'] as const;
function opaqueId(value: unknown) {
  const result = text(value);
  if (!result.trim() || new TextEncoder().encode(result).length > 256 || /\p{Cc}/u.test(result))
    throw new ProtocolError('invalid opaque source identifier');
  return result;
}
const targetValue = tagged(
  {
    available: closed({
      status: enumeration('available'),
      ids,
      present: array(enumeration(...resources)),
    }),
    reserved: closed({ status: enumeration('reserved'), ids }),
    redacted: closed({ status: enumeration('redacted') }),
  },
  'status',
);
const source = tagged(
  {
    available: closed({
      status: enumeration('available'),
      platform: enumeration('telegram', 'telegram-new', 'discord', 'lark', 'feishu'),
      chat_id: opaqueId,
      sender_id: opaqueId,
      sender_display_name: nullable(label),
      conversation_type: enumeration('private', 'group', 'channel', 'unknown'),
    }),
    unavailable: closed({ status: enumeration('unavailable') }),
    redacted: closed({ status: enumeration('redacted') }),
  },
  'status',
);
const partitionValue = closed({
  id: allocationId,
  bridge_id: boundedId,
  generation,
  revision: positiveInteger,
  config_revision: generation,
  current: boolean,
  state: enumeration(
    'reserved',
    'provisioning',
    'ready',
    'needs_connection',
    'failed_retryable',
    'failed_terminal',
    'retired',
  ),
  placement: enumeration(
    'existing_agent',
    'new_sandbox',
    'workspace_per_partition',
    'worktree_agent_per_partition',
  ),
  context_sharing: enumeration(
    'chat_sender',
    'chat',
    'thread_sender',
    'thread',
    'sender_across_chats',
    'fixed_agent',
  ),
  target: targetValue,
  source,
  observed_at_ms: integer,
});
export function partition(value: unknown) {
  const result = partitionValue(value);
  if ((result.target.status === 'redacted') !== (result.source.status === 'redacted'))
    throw new ProtocolError('partition redaction states disagree');
  if (result.target.status === 'available') {
    const present = result.target.present;
    if (
      !present.length ||
      present.length > 4 ||
      present.some(
        (kind, i) => i > 0 && resources.indexOf(kind) <= resources.indexOf(present[i - 1]!),
      )
    )
      throw new ProtocolError('partition resources are not bounded, unique and ordered');
  }
  return result;
}
export type BridgePartition = Output<typeof partition>;
export type BridgePartitionTarget = BridgePartition['target'];
export type BridgePartitionSource = BridgePartition['source'];

const terminal = (status: string) => ['completed', 'failed', 'cancelled'].includes(status);
const commandValue = closed({
  command_id: boundedId,
  response_id: boundedId,
  config_revision: generation,
  agent_order: positiveInteger,
  status: enumeration(
    'queued',
    'in_progress',
    'waiting_action',
    'completed',
    'failed',
    'cancelled',
  ),
  waiting_for: enumeration('none', 'worker', 'approval', 'action'),
  turn_sequence: nullable(positiveInteger),
  delivery: nullable(
    enumeration(
      'dispatch_fenced',
      'write_confirmed',
      'execution_observed',
      'pre_write_failed',
      'recovery_required',
      'runtime_lost',
    ),
  ),
  admitted_at: text,
});
function command(value: unknown) {
  const result = commandValue(value);
  if (
    !result.admitted_at.trim() ||
    /\p{Cc}/u.test(result.admitted_at) ||
    result.admitted_at.length > 64 ||
    (result.status === 'waiting_action') !== (result.waiting_for !== 'none') ||
    (result.waiting_for === 'worker' &&
      (result.turn_sequence !== null || result.delivery !== null)) ||
    (result.waiting_for === 'approval' && result.turn_sequence === null)
  )
    throw new ProtocolError('inconsistent partition command status');
  return result;
}
export type BridgePartitionCommand = Output<typeof command>;
const activityValue = tagged(
  {
    available: closed({
      status: enumeration('available'),
      agent_status: enumeration(
        'starting',
        'ready',
        'working',
        'waiting_approval',
        'paused',
        'stopped',
        'deleted',
        'failed',
      ),
      pending: array(command),
      pending_truncated: boolean,
      source_unavailable: boolean,
      latest: nullable(command),
      observed_at_ms: integer,
    }),
    unavailable: closed({ status: enumeration('unavailable') }),
    redacted: closed({ status: enumeration('redacted') }),
  },
  'status',
);
const detailValue = closed({ partition, activity: activityValue });
export function detail(value: unknown) {
  const result = detailValue(value);
  if ((result.partition.target.status === 'redacted') !== (result.activity.status === 'redacted'))
    throw new ProtocolError('partition activity redaction disagrees');
  const activity = result.activity;
  if (activity.status === 'available') {
    if (
      result.partition.target.status !== 'available' ||
      !result.partition.target.present.includes('agent')
    )
      throw new ProtocolError('partition activity requires an accessible Agent');
    const pending = activity.pending;
    if (
      pending.length > 64 ||
      pending.some(
        (item, i) =>
          terminal(item.status) || (i > 0 && item.agent_order <= pending[i - 1]!.agent_order),
      ) ||
      new Set(pending.map((item) => item.command_id)).size !== pending.length ||
      new Set(pending.map((item) => item.response_id)).size !== pending.length ||
      (activity.pending_truncated && pending.length < 64 && !activity.source_unavailable)
    )
      throw new ProtocolError('invalid bounded pending command set');
    if (activity.latest === null && pending.length && !activity.source_unavailable)
      throw new ProtocolError('partition latest response is missing');
    if (activity.latest !== null) {
      if (pending.some((item) => item.agent_order > activity.latest!.agent_order))
        throw new ProtocolError('partition latest response predates pending work');
      const same = pending.find((item) => item.command_id === activity.latest!.command_id);
      if (same !== undefined && JSON.stringify(same) !== JSON.stringify(activity.latest))
        throw new ProtocolError('partition command observations disagree');
    }
  }
  return result;
}
export type BridgePartitionDetail = Output<typeof detail>;
export type BridgePartitionActivity = BridgePartitionDetail['activity'];
const pendingMessage = closed({
  command_id: boundedId,
  response_id: boundedId,
  agent_order: positiveInteger,
  admitted_at: text,
  platform: enumeration('telegram', 'telegram-new', 'discord', 'lark', 'feishu'),
  text,
  text_truncated: boolean,
});
const pendingMessagesValue = tagged(
  {
    available: closed({
      status: enumeration('available'),
      bridge_id: boundedId,
      partition_id: allocationId,
      generation,
      agent_id: boundedId,
      through_sequence: integer,
      messages: array(pendingMessage),
      pending_truncated: boolean,
      source_unavailable: boolean,
      observed_at_ms: integer,
    }),
    unavailable: closed({ status: enumeration('unavailable') }),
    redacted: closed({ status: enumeration('redacted') }),
  },
  'status',
);
export function pendingMessages(value: unknown) {
  const result = pendingMessagesValue(value);
  if (
    result.status === 'available' &&
    (result.through_sequence < 0 ||
      result.observed_at_ms < 0 ||
      result.messages.length > 64 ||
      new Set(result.messages.map((item) => item.command_id)).size !== result.messages.length ||
      new Set(result.messages.map((item) => item.response_id)).size !== result.messages.length ||
      result.messages.some(
        (item, i) =>
          [...item.text].length > 1024 ||
          !Number.isFinite(Date.parse(item.admitted_at)) ||
          (i > 0 && item.agent_order <= result.messages[i - 1]!.agent_order),
      ))
  )
    throw new ProtocolError('invalid bounded pending Channel messages');
  return result;
}
export type PendingChannelMessages = Output<typeof pendingMessages>;
export type PendingChannelMessage = Output<typeof pendingMessage>;
export const page = closed({ items: array(partition), next_cursor: nullable(cursor) });
export type BridgePartitionPage = Output<typeof page>;

export function cursorAfter(value: string, bridge: string): string {
  try {
    cursor(value);
    const raw = atob(value.replaceAll('-', '+').replaceAll('_', '/'));
    const decoded: unknown = JSON.parse(
      new TextDecoder('utf-8', { fatal: true }).decode(
        Uint8Array.from(raw, (c) => c.charCodeAt(0)),
      ),
    );
    if (
      !Array.isArray(decoded) ||
      decoded.length !== 3 ||
      decoded[0] !== 'cma.bridge.partitions.v1' ||
      decoded[1] !== bridge
    )
      throw new Error('scope');
    const id = allocationId(decoded[2]);
    const canonical = btoa(
      String.fromCharCode(...new TextEncoder().encode(JSON.stringify(decoded))),
    )
      .replaceAll('+', '-')
      .replaceAll('/', '_')
      .replaceAll('=', '');
    if (canonical !== value) throw new Error('encoding');
    return id;
  } catch (cause) {
    throw new ProtocolError('partition cursor belongs to a different scope or is invalid', {
      cause,
    });
  }
}
