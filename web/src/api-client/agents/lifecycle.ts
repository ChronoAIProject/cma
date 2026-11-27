import { taskTemplate } from './task-template.js';
import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import { command, identity, input, read, segment, type CommandOptions } from '../resources.js';
import { agent } from './types.js';
import { sandboxQuotaUsage } from '../instances/capacity.js';
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

export const githubRepositoryNeed = enumeration(
  'public_repository_write',
  'private_repository_read',
);

export const agentLifecycleCreateRequest = object({
  name: text,
  title_mode: optional(enumeration('automatic', 'named')),
  initial_message: optional(text),
  template: optional(taskTemplate),
  desktop: optional(boolean),
  machine_included: optional(boolean),
  workspace_id: optional(text),
  sandbox_id: optional(text),
  image: optional(object({ id: text, version: optional(text) })),
  repository: optional(
    object({
      url: text,
      ref: optional(text),
      operation: optional(githubRepositoryNeed),
      connection_id: optional(text),
    }),
  ),
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

export const agentLifecycleFailure = object({
  code: enumeration('sandbox_limit_reached'),
  limit: integer,
  live: integer,
  quota_usage: optional(sandboxQuotaUsage),
});
export type AgentLifecycleFailure = Output<typeof agentLifecycleFailure>;

export const repositoryAccessRefusal = enumeration(
  'verification_unavailable',
  'insufficient_scope',
  'missing_connection',
  'repository_unavailable',
  'reference_unavailable',
  'runtime_unsupported',
);
export type RepositoryAccessRefusal = Output<typeof repositoryAccessRefusal>;

export const repositoryScopeRepair = object({
  connection_id: text,
  operation: githubRepositoryNeed,
  required_scope: enumeration('public_repo', 'repo'),
  manage_url: text,
});
export type RepositoryScopeRepair = Output<typeof repositoryScopeRepair>;

export const agentLifecycleOperation = object({
  callback_url: optional(nullable(text)),
  id: text,
  accepted_at: optional((value) => {
    const time = text(value);
    if (!Number.isFinite(Date.parse(time))) throw new ProtocolError('invalid acceptance time');
    return time;
  }),
  status: enumeration('queued', 'running', 'waiting_for_connection', 'succeeded', 'failed'),
  phase: text,
  can_cancel: optional(boolean),
  agent_id: optional(text),
  error: optional(text),
  failure: optional(agentLifecycleFailure),
  events: array(agentLifecycleEvent),
  connection: optional(
    object({
      services: array(text),
      target: agentLifecycleConnectionTarget,
      repository_access: optional(repositoryAccessRefusal),
      repository_scope_repair: optional(repositoryScopeRepair),
    }),
  ),
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

export function cancelAgentLifecycle(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions,
) {
  return read(
    config,
    `/api/v1/agent-lifecycle/${segment(id)}/cancel`,
    identity(agentLifecycleOperation, id),
    {
      ...options,
      method: 'POST',
    },
  );
}

export function recoverAgentWorktree(config: ResolvedConfig, id: string, options: CommandOptions) {
  return read(config, `/api/v1/agents/${segment(id)}/recover-worktree`, identity(agent, id), {
    ...command(options),
    method: 'POST',
    expectedStatus: 202,
  });
}
