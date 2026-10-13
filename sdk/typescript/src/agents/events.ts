import { agentStatus, failure } from '../instances/types.js';
import {
  array,
  dictionary,
  enumeration,
  integer,
  object,
  optional,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import { firstMessageDelivery } from './types.js';

export const agentWorkStep = enumeration(
  'add_worktree',
  'materialize_home',
  'open_conversation',
  'publish_ready',
  'publish_connection',
  'dispatch_first_message',
  'fence_interaction',
  'close_process',
  'publish_stopped',
  'ensure_process_closed',
  'remove_home',
  'remove_worktree',
  'publish_deleted',
  'none',
);
export type AgentWorkStep = Output<typeof agentWorkStep>;
export const agentOperationKind = enumeration(
  'send_message',
  'steer',
  'interrupt',
  'answer_approval',
  'update_settings',
);
export type AgentOperationKind = Output<typeof agentOperationKind>;
export const agentOperationDelivery = enumeration(
  'dispatch_fenced',
  'write_confirmed',
  'execution_observed',
  'pre_write_failed',
  'recovery_required',
  'runtime_lost',
);
export type AgentOperationDelivery = Output<typeof agentOperationDelivery>;
export const agentEventKind = tagged({
  access_redacted: variant('access_redacted', {}),
  created: variant('created', { workspace_id: text, worktree_id: text }),
  conversation_opened: variant('conversation_opened', {
    conversation_id: text,
    runtime_generation: integer,
  }),
  runtime_generation_retired: variant('runtime_generation_retired', {
    previous_generation: integer,
    runtime_generation: integer,
  }),
  work_step_completed: variant('work_step_completed', { step: agentWorkStep }),
  work_step_failed: variant('work_step_failed', { step: agentWorkStep, failure }),
  turn_started: variant('turn_started', { turn_sequence: integer }),
  turn_metadata_set: variant('turn_metadata_set', {
    turn_sequence: integer,
    metadata: dictionary(text),
  }),
  owner_message: variant('owner_message', { content: text }),
  agent_message: variant('agent_message', { content: text }),
  turn_completed: variant('turn_completed', { turn_sequence: integer }),
  turn_failed: variant('turn_failed', { turn_sequence: integer, failure }),
  turn_interrupted: variant('turn_interrupted', { turn_sequence: integer }),
  tool_call_started: variant('tool_call_started', {
    call_id: text,
    title: text,
    kind: text,
    status: text,
  }),
  tool_call_updated: variant('tool_call_updated', {
    call_id: text,
    status: text,
    title: optional(text),
  }),
  approval_requested: variant('approval_requested', {
    approval_id: text,
    action: optional(text),
    detail: optional(text),
  }),
  approval_resolved: variant('approval_resolved', { approval_id: text, decision: text }),
  operation_delivery_changed: variant('operation_delivery_changed', {
    operation_sequence: integer,
    operation: agentOperationKind,
    state: agentOperationDelivery,
  }),
  usage: variant('usage', {
    input_tokens: integer,
    output_tokens: integer,
    context_used: integer,
    context_window: optional(integer),
  }),
  mode_changed: variant('mode_changed', { mode: text }),
  plan_set: variant('plan_set', { entries: array(object({ title: text, status: text })) }),
  config_options_set: variant('config_options_set', {
    options: array(object({ id: text, name: text, kind: text })),
  }),
  available_commands_set: variant('available_commands_set', {
    commands: array(object({ name: text, description: text })),
  }),
  conversation_gap: variant('conversation_gap', { frames: integer }),
  sign_in_required: variant('sign_in_required', {
    url: text,
    code: optional(text),
    message: optional(text),
  }),
  status_changed: variant('status_changed', { from: agentStatus, to: agentStatus }),
  first_message_changed: variant('first_message_changed', {
    state: firstMessageDelivery,
    recovery_generation: integer,
  }),
  title_set: variant('title_set', { title: text, source: text }),
  stopped: variant('stopped', { reason: text }),
  deleted: variant('deleted', {}),
});
export type AgentEventKind = Output<typeof agentEventKind>;
export const agentEvent = object({ seq: integer, appended_at: text, kind: agentEventKind });
export type AgentEvent = Output<typeof agentEvent>;
