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
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  record,
  tagged,
  text,
  type Output,
} from './schema.js';

export const WorkerCredentialDigestSchema = object({ sha256: text });
export type WorkerCredentialDigest = Output<typeof WorkerCredentialDigestSchema>;
export const WorkerCredentialClassificationSchema = object({
  classification: enumeration('known_worker', 'unknown'),
});
export type WorkerCredentialClassification = Output<typeof WorkerCredentialClassificationSchema>;

export const BindingTargetSchema = tagged(
  {
    profile: object({ type: enumeration('profile') }),
    existing_agent: object({ type: enumeration('existing_agent'), agent_id: text }),
  },
  'type',
);
export type BindingTarget = Output<typeof BindingTargetSchema>;

const binding = object({
  queue_lifetime_seconds: optional(positiveInteger),
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
export const BindingConversationInspectionSchema = object({
  binding_id: text,
  generation: positiveInteger,
  incarnation: integer,
  conversation_id: text,
  agent_id: text,
  state: text,
  agent_status: nullable(text),
  profile_drift: optional(
    object({ assigned_profile_digest: text, prompt: boolean, skills: boolean, model: boolean }),
  ),
});
export type BindingConversationInspection = Output<typeof BindingConversationInspectionSchema>;
const bindingConversationCatalog = object({
  binding_id: text,
  generation: positiveInteger,
  revision: positiveInteger,
  transport_conversations: array(text),
});
export type BindingConversationCatalog = Output<typeof bindingConversationCatalog>;
const inspectBindingProfile = object({ profile: record, profile_digest: text });
export type InspectBindingProfile = Output<typeof inspectBindingProfile>;
const desiredBindingProfileDrift = object({
  profile_digest: text,
  prompt: boolean,
  skills: boolean,
  model: boolean,
  service_scope: boolean,
});
export type DesiredBindingProfileDrift = Output<typeof desiredBindingProfileDrift>;
const transferBindingProfile = object({
  destination_binding_id: text,
  expected_agent_id: text,
  expected_generation: positiveInteger,
  expected_destination_generation: positiveInteger,
  expected_incarnation: integer,
  owner: text,
  organization: boolean,
  request_digest: text,
});
export type TransferBindingProfile = Output<typeof transferBindingProfile>;
const bindingProfileTransfer = object({
  source_binding_id: text,
  source_generation: positiveInteger,
  source_incarnation: integer,
  source_agent_id: text,
  source_conversation_id: text,
  conversation_id: text,
  destination_binding_id: text,
  destination_generation: positiveInteger,
  destination_agent_id: text,
  destination_worktree_id: text,
  state: enumeration('reserved'),
});
export type BindingProfileTransfer = Output<typeof bindingProfileTransfer>;
export const ResetBindingConversationSchema = object({
  expected_agent_id: text,
  owner: text,
  organization: boolean,
  idempotency_binding_id: text,
  request_digest: text,
});
export type ResetBindingConversation = Output<typeof ResetBindingConversationSchema>;
export const BindingConversationResetSchema = object({
  binding_id: text,
  generation: positiveInteger,
  conversation_id: text,
  incarnation: positiveInteger,
  previous_agent_id: text,
  previous_conversation_id: text,
  successor_agent_id: text,
  state: enumeration('reserved'),
});
export type BindingConversationReset = Output<typeof BindingConversationResetSchema>;
export type PutProviderBinding = Output<typeof putBinding>;
/** Binding management requires Gateway authority; media operations use the bound consumer key. */
export class ProviderBindings {
  readonly media: BindingEventMedia;
  constructor(private readonly config: ResolvedConfig) {
    this.media = new BindingEventMedia(config);
  }
  /** Gateway service-account bearer only; errors never become an unknown result. */
  classifyWorkerCredential(body: WorkerCredentialDigest, options: OperationOptions = {}) {
    return read(
      this.config,
      '/api/v1/cmaeg/worker-credentials/classify',
      WorkerCredentialClassificationSchema,
      {
        ...options,
        method: 'POST',
        body: input(WorkerCredentialDigestSchema, body),
      },
    );
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
  listConversations(id: string, options: OperationOptions = {}) {
    return read(this.config, `${path(id)}/conversations`, bindingConversationCatalog, options);
  }
  inspectConversation(id: string, conversationId: string, options: OperationOptions = {}) {
    return read(
      this.config,
      path(id, conversationId),
      BindingConversationInspectionSchema,
      options,
    );
  }
  inspectDesiredProfile(
    id: string,
    conversationId: string,
    body: InspectBindingProfile,
    options: OperationOptions = {},
  ) {
    return read(
      this.config,
      `${path(id, conversationId)}/profile-drift`,
      nullable(desiredBindingProfileDrift),
      {
        ...options,
        method: 'POST',
        body: input(inspectBindingProfile, body),
      },
    );
  }
  /** Retain the original transfer request and operation key for any explicit retry. */
  transferConversationProfile(
    id: string,
    conversationId: string,
    body: TransferBindingProfile,
    options: CommandOptions,
  ) {
    return read(
      this.config,
      `${path(id, conversationId)}/profile-transfers`,
      bindingProfileTransfer,
      {
        ...command(options),
        method: 'POST',
        body: input(transferBindingProfile, body),
      },
    );
  }
  ensureConversation(id: string, conversationId: string, options: OperationOptions = {}) {
    return read(this.config, path(id, conversationId), conversation, {
      ...options,
      method: 'PUT',
      expectedStatus: [200, 201],
    });
  }
  /** Retain the original request and operation key for an explicit retry. */
  resetConversation(
    id: string,
    conversationId: string,
    body: ResetBindingConversation,
    options: CommandOptions,
  ) {
    return read(this.config, `${path(id, conversationId)}/resets`, BindingConversationResetSchema, {
      ...command(options),
      method: 'POST',
      body: input(ResetBindingConversationSchema, body),
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
