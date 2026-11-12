import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { command, identity, input, read, segment, type CommandOptions } from '../resources.js';
import { agent } from './types.js';
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
  variant,
  type Output,
} from '../schema.js';

export const agentLifecycleCreateRequest = object({
  name: text,
  initial_message: optional(text),
  desktop: optional(boolean),
  machine_included: optional(boolean),
  workspace_id: optional(text),
  sandbox_id: optional(text),
  /** Retained for compatibility; the server rejects custom image selection with image_selection_unsupported. */
  image: optional(object({ id: text, version: optional(text) })),
  repository: optional(object({ url: text, ref: optional(text) })),
  profile: optional(
    object({
      model: optional(text),
      reasoning_effort: optional(enumeration('minimal', 'low', 'medium', 'high', 'x_high')),
      permission_mode: optional(enumeration('guarded', 'autonomous')),
      system_prompt: optional(text),
      skills: optional(array(object({ id: text, version: optional(text) }))),
      permissions: optional(object({ services: optional(array(text)) })),
    }),
  ),
});
/** Inline configuration; omitted image and profile options use deployment defaults. */
export type AgentLifecycleCreateRequest = Output<typeof agentLifecycleCreateRequest>;

export const agentLifecycleCreateReceipt = object({
  id: text,
  status: enumeration('provisioning'),
});
/** Durable admission receipt. Poll the operation before opening its Agent. */
export type AgentLifecycleCreateReceipt = Output<typeof agentLifecycleCreateReceipt>;

export const agentLifecycleConnectionTarget = tagged({
  sandbox_profile: variant('sandbox_profile', {
    profile: object({ id: text, revision: positiveInteger }),
  }),
  sandbox: variant('sandbox', { id: text }),
  workspace: variant('workspace', { id: text }),
  agent: variant('agent', { id: text }),
  launch: variant('launch', { id: text }),
});
/** Owner-bound target for a service connection recovery flow. */
export type AgentLifecycleConnectionTarget = Output<typeof agentLifecycleConnectionTarget>;

export const agentLifecycleEvent = object({
  sequence: integer,
  phase: text,
  status: text,
  message: text,
  occurred_at: text,
});
export type AgentLifecycleEvent = Output<typeof agentLifecycleEvent>;

export const agentLifecycleOperation = object({
  callback_url: optional(nullable(text)),
  id: text,
  status: enumeration('queued', 'running', 'waiting_for_connection', 'succeeded', 'failed'),
  phase: text,
  agent_id: optional(text),
  error: optional(text),
  events: array(agentLifecycleEvent),
  connection: optional(object({ services: array(text), target: agentLifecycleConnectionTarget })),
});
/** Phase readiness is separate from the terminal operation status. */
export type AgentLifecycleOperation = Output<typeof agentLifecycleOperation>;

export function createAgentLifecycle(
  config: ResolvedConfig,
  body: AgentLifecycleCreateRequest,
  options: CommandOptions,
) {
  return read(config, '/api/v1/agents', agentLifecycleCreateReceipt, {
    ...command(options),
    method: 'POST',
    body: input(agentLifecycleCreateRequest, body),
    expectedStatus: 202,
  });
}

export function readAgentLifecycle(config: ResolvedConfig, id: string, options: OperationOptions) {
  return read(
    config,
    `/api/v1/agent-lifecycle/${segment(id)}`,
    identity(agentLifecycleOperation, id),
    options,
  );
}

export function recoverAgentWorktree(config: ResolvedConfig, id: string, options: CommandOptions) {
  return read(config, `/api/v1/agents/${segment(id)}/recover-worktree`, identity(agent, id), {
    ...command(options),
    method: 'POST',
    expectedStatus: 202,
  });
}
