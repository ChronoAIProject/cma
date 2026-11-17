import {
  ActionModeSchema,
  ApiTypeSchema,
  ModelAccessSchema,
  NyxIdIdentityKindSchema,
  ProviderSlugSchema,
  SkillSourceSchema,
  ThinkingEffortSchema,
} from './primitives.js';
import {
  array,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  type Output,
} from '../../schema.js';
import { anything } from './helpers.js';

export const McpAuthSchema = tagged(
  {
    none: object({ type: enumeration('none') }),
    nyxid_service: object({ slug: text, type: enumeration('nyxid_service') }),
  },
  'type',
);
export type McpAuth = Output<typeof McpAuthSchema>;

export const McpServerRefSchema = object({ auth: McpAuthSchema, url: text });
export type McpServerRef = Output<typeof McpServerRefSchema>;

export const SkillRefSchema = object({
  id: text,
  source: SkillSourceSchema,
  version: optional(nullable(text)),
});
export type SkillRef = Output<typeof SkillRefSchema>;

export const ContextConfigSchema = object({
  mcp_servers: optional(array(McpServerRefSchema)),
  skills: optional(array(SkillRefSchema)),
  system_prompt: optional(text),
});
export type ContextConfig = Output<typeof ContextConfigSchema>;

export const NyxIdIdentitySchema = object({ kind: NyxIdIdentityKindSchema, subject: text });
export type NyxIdIdentity = Output<typeof NyxIdIdentitySchema>;

export const ProfileMetadataSchema = object({
  description: optional(text),
  name: text,
  owner: NyxIdIdentitySchema,
  provider: ProviderSlugSchema,
});
export type ProfileMetadata = Output<typeof ProfileMetadataSchema>;

export const ModelConfigSchema = object({
  access: ModelAccessSchema,
  api_type: ApiTypeSchema,
  model: text,
  service: text,
  thinking_effort: ThinkingEffortSchema,
});
export type ModelConfig = Output<typeof ModelConfigSchema>;

export const PermissionConfigSchema = object({
  action_mode: ActionModeSchema,
  service_scope: optional(array(text)),
});
export type PermissionConfig = Output<typeof PermissionConfigSchema>;

export const EventWorkerProfileSchema = object({
  context: ContextConfigSchema,
  environment: optional(anything),
  metadata: ProfileMetadataSchema,
  model: ModelConfigSchema,
  permission: PermissionConfigSchema,
  queue_lifetime_seconds: optional(integer),
  schema_version: integer,
});
export type EventWorkerProfile = Output<typeof EventWorkerProfileSchema>;

/** Compatibility aliases for the original protocol 1 SDK names. */
export const AgentProfileSnapshotSchema = EventWorkerProfileSchema;
export type AgentProfileSnapshot = EventWorkerProfile;
