import type { ResolvedConfig } from './config.js';
import { requestVoid, type OperationOptions } from './http.js';
import { input, read, segment } from './resources.js';
import {
  array,
  enumeration,
  object,
  optional,
  positiveInteger,
  record,
  tagged,
  text,
  type Output,
} from './schema.js';

export const BindingTargetSchema = tagged(
  {
    profile: object({ type: enumeration('profile') }),
    existing_agent: object({ type: enumeration('existing_agent'), agent_id: text }),
  },
  'type',
);
export type BindingTarget = Output<typeof BindingTargetSchema>;

const binding = object({
  target: optional(BindingTargetSchema),
  binding_id: text,
  owner: text,
  profile_digest: text,
  sandbox_id: text,
  workspace_id: text,
  level: text,
  state: enumeration('provisioning', 'ready', 'retired', 'failed'),
  reason: optional(text),
  honoured: array(text),
  refused: array(object({ field: text, reason: text })),
  generation: positiveInteger,
  created_at: text,
  updated_at: text,
});
const conversation = object({
  conversation_id: text,
  agent_id: optional(text),
  state: enumeration('provisioning', 'ready', 'failed', 'retiring', 'retired'),
});
const putBinding = object({
  target: optional(BindingTargetSchema),
  schema_version: positiveInteger,
  profile: record,
  profile_digest: text,
  agent_key: text,
  tools: object({ native_mcp_url: text, nyxid_service_slug: text }),
});
export type ProviderBinding = Output<typeof binding>;
export type BindingConversation = Output<typeof conversation>;
export type PutProviderBinding = Output<typeof putBinding>;
/** Gateway management authority only; binding keys use Responses and context reads. */
export class ProviderBindings {
  constructor(private readonly config: ResolvedConfig) {}
  put(id: string, body: PutProviderBinding, options: OperationOptions = {}) {
    return read(this.config, path(id), binding, {
      ...options,
      method: 'PUT',
      body: input(putBinding, body),
    });
  }
  get(id: string, options: OperationOptions = {}) {
    return read(this.config, path(id), binding, options);
  }
  delete(id: string, options: OperationOptions = {}) {
    return requestVoid(this.config, path(id), {
      ...options,
      method: 'DELETE',
      expectedStatus: 204,
    });
  }
  ensureConversation(id: string, conversationId: string, options: OperationOptions = {}) {
    return read(this.config, path(id, conversationId), conversation, {
      ...options,
      method: 'PUT',
      expectedStatus: [200, 201],
    });
  }
  deleteConversation(id: string, conversationId: string, options: OperationOptions = {}) {
    return requestVoid(this.config, path(id, conversationId), {
      ...options,
      method: 'DELETE',
      expectedStatus: 204,
    });
  }
}
function path(id: string, conversationId?: string) {
  const base = `/api/v1/cmaeg/bindings/${segment(id)}`;
  return conversationId === undefined ? base : `${base}/conversations/${segment(conversationId)}`;
}
