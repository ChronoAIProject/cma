import { agentOperationDelivery, agentOperationKind, agentWorkStep } from '../agents/events.js';
import { firstMessageDelivery, agentRuntimeMetadata } from '../agents/types.js';
import { agentStatus } from '../instances/types.js';
import {
  array,
  boolean,
  dictionary,
  enumeration,
  integer,
  nullable,
  optional,
  positiveInteger,
  tagged,
  text,
  type Output,
  type Parser,
} from '../schema.js';
import { externalApprovalKind, type ExternalApprovalKind } from './external-approval.js';
import { record } from '../schema.js';
import { strict } from './schema.js';

const failure = strict({ class: text, detail: text });
function kind<
  const Name extends string,
  const Fields extends Readonly<Record<string, Parser<unknown>>>,
>(name: Name, fields: Fields) {
  return strict({ type: enumeration(name), ...fields });
}
/** Safe common Agent event vocabulary. It contains no runtime locator or hidden reasoning. */
export const v2AgentEventKind = tagged({
  created: kind('created', { workspace_id: text, worktree_id: text }),
  conversation_available: kind('conversation_available', {}),
  runtime_changed: kind('runtime_changed', {}),
  work_completed: kind('work_completed', { step: agentWorkStep }),
  work_failed: kind('work_failed', { step: agentWorkStep, failure }),
  skill_installation_changed: kind('skill_installation_changed', {
    skill_id: text,
    status: enumeration('pending', 'installed', 'failed'),
  }),
  turn_started: kind('turn_started', { turn_sequence: positiveInteger }),
  turn_metadata: kind('turn_metadata', {
    turn_sequence: positiveInteger,
    metadata: dictionary(text),
  }),
  message: kind('message', { role: enumeration('user', 'assistant'), content: text }),
  turn_completed: kind('turn_completed', { turn_sequence: positiveInteger }),
  turn_failed: kind('turn_failed', { turn_sequence: positiveInteger, failure }),
  turn_cancelled: kind('turn_cancelled', { turn_sequence: positiveInteger }),
  tool_started: kind('tool_started', { call_id: text, title: text, kind: text, status: text }),
  tool_updated: kind('tool_updated', {
    call_id: text,
    status: text,
    title: nullable(text),
    output: optional(strict({ text, truncated: boolean })),
  }),
  tool_output_reported: kind('tool_output_reported', {
    call_id: text,
    output: strict({ text, truncated: boolean }),
  }),
  approval_requested: kind('approval_requested', {
    approval_id: text,
    action: nullable(text),
    detail: nullable(text),
  }),
  approval_resolved: kind('approval_resolved', { approval_id: text, decision: text }),
  delivery_changed: kind('delivery_changed', {
    operation_sequence: positiveInteger,
    operation: agentOperationKind,
    state: agentOperationDelivery,
  }),
  usage: kind('usage', {
    input_tokens: integer,
    output_tokens: integer,
    context_used: integer,
    context_window: nullable(integer),
  }),
  runtime_metadata_changed: kind('runtime_metadata_changed', { metadata: agentRuntimeMetadata }),
  usage_updated: kind('usage_updated', {
    input_tokens: integer,
    output_tokens: integer,
    context_used: integer,
    context_window: nullable(integer),
    turn_sequence: nullable(integer),
    turn_input_tokens: integer,
    turn_output_tokens: integer,
  }),
  mode_changed: kind('mode_changed', { mode: text }),
  plan_set: kind('plan_set', { entries: array(strict({ title: text, status: text })) }),
  config_options_set: kind('config_options_set', {
    options: array(strict({ id: text, name: text, kind: text })),
  }),
  available_commands_set: kind('available_commands_set', {
    commands: array(strict({ name: text, description: text })),
  }),
  history_gap: kind('history_gap', { frames: integer }),
  authentication_required: kind('authentication_required', {}),
  status_changed: kind('status_changed', { from: agentStatus, to: agentStatus }),
  first_message_changed: kind('first_message_changed', { state: firstMessageDelivery }),
  title_set: kind('title_set', { title: text, source: text }),
  stopped: kind('stopped', { reason: text }),
  deleted: kind('deleted', {}),
});
export type V2AgentEventKind = Output<typeof v2AgentEventKind> | ExternalApprovalKind;

export function negotiatedAgentEventKind(
  value: unknown,
  externalApprovals = false,
): V2AgentEventKind {
  const type = record(value).type;
  return externalApprovals &&
    (type === 'external_approval_requested' || type === 'external_approval_resolved')
    ? externalApprovalKind(value)
    : v2AgentEventKind(value);
}
