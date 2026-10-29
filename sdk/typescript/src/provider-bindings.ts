import type { ResolvedConfig } from './config.js';
import { BindingEventMedia } from './provider-binding-media.js';
export * from './provider-binding-media.js';
import { requestVoid, type OperationOptions } from './http.js';
import { command, input, read, segment, type CommandOptions } from './resources.js';
import {
  BindingPreparationInspectionSchema,
  BindingPreparationReceiptSchema,
  InspectBindingPreparationSchema,
  PrepareBindingSchema,
  type InspectBindingPreparation,
  type PrepareBinding,
} from './provider-binding-preparation.js';
export * from './provider-binding-preparation.js';
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
/** Binding management requires Gateway authority; media operations use the bound consumer key. */
export class ProviderBindings {
  readonly media: BindingEventMedia;
  constructor(private readonly config: ResolvedConfig) {
    this.media = new BindingEventMedia(config);
  }
  /** Inspect the current generation without allocating or mutating resources. */
  inspectPreparation(id: string, body: InspectBindingPreparation, options: OperationOptions = {}) {
    return read(
      this.config,
      `${path(id)}/preparation-inspections`,
      BindingPreparationInspectionSchema,
      {
        ...options,
        method: 'POST',
        body: input(InspectBindingPreparationSchema, body),
      },
    );
  }
  /** Reserve with the exact retained intent and operation key; never invent a key on retry. */
  prepare(id: string, body: PrepareBinding, options: CommandOptions) {
    return read(this.config, `${path(id)}/preparations`, BindingPreparationReceiptSchema, {
      ...command(options),
      method: 'POST',
      body: input(PrepareBindingSchema, body),
    });
  }
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
