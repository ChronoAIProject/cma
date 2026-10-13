import {
  array,
  enumeration,
  integer,
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
  workspace_id: optional(text),
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
