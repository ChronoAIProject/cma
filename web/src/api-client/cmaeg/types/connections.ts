import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../../schema.js';

export const StatusSchema = enumeration(
  'draft',
  'awaiting_platform',
  'ready',
  'cancelled',
  'expired',
);
export type Status = Output<typeof StatusSchema>;

export const PlatformCapabilitySchema = object({
  id: text,
  label: text,
  connection_methods: array(text),
  optional_capabilities: array(text),
});
export type PlatformCapability = Output<typeof PlatformCapabilitySchema>;

export const RelayVerificationCapabilitySchema = object({
  id: text,
  writes_enabled: boolean,
});
export type RelayVerificationCapability = Output<typeof RelayVerificationCapabilitySchema>;
export const RelayPlatformCapabilitySchema = object({
  id: text,
  label: text,
  verification_modes: array(RelayVerificationCapabilitySchema),
});
export type RelayPlatformCapability = Output<typeof RelayPlatformCapabilitySchema>;
export const RelayAuthoringCapabilitiesSchema = object({
  custom_durable_execution_enabled: optional(nullable(boolean)),
  response_recovery_providers: optional(nullable(array(text))),
  native_telegram_message_events_enabled: optional(nullable(boolean)),
  native_telegram_member_joins_enabled: optional(nullable(boolean)),
  relay_telegram_member_joins_enabled: optional(nullable(boolean)),
  native_discord_thread_history_enabled: optional(nullable(boolean)),
  native_telegram_topic_lifecycle_enabled: optional(nullable(boolean)),
  native_telegram_topic_creation_enabled: optional(nullable(boolean)),
  native_telegram_general_topic_lifecycle_enabled: optional(nullable(boolean)),
  native_discord_thread_lifecycle_enabled: optional(nullable(boolean)),
  native_lark_conversation_history_enabled: optional(nullable(boolean)),
  native_discord_conversation_history_enabled: optional(nullable(boolean)),
  native_discord_reactions_enabled: optional(nullable(boolean)),
  native_discord_rich_content_enabled: optional(nullable(boolean)),
  native_discord_forwards_enabled: optional(nullable(boolean)),
  native_discord_writes_enabled: optional(nullable(boolean)),
  native_discord_auto_activation_enabled: optional(nullable(boolean)),
  native_discord_capacity: optional(nullable(integer)),
  native_lark_writes_enabled: optional(nullable(boolean)),
  native_lark_intake_enabled: optional(nullable(boolean)),
  native_lark_controls_enabled: optional(nullable(boolean)),
  native_whatsapp_forms_enabled: optional(nullable(boolean)),
  native_whatsapp_message_events_enabled: optional(nullable(boolean)),
  native_whatsapp_flow_writes_enabled: optional(nullable(boolean)),
  native_lark_forms_enabled: optional(nullable(boolean)),
  native_whatsapp_writes_enabled: optional(nullable(boolean)),
  native_whatsapp_groups_writes_enabled: optional(nullable(boolean)),
  native_whatsapp_auto_activation_enabled: optional(nullable(boolean)),
  dynamic_sources_writes_enabled: boolean,
  relay_raw_verification_writes_enabled: boolean,
  lark_resource_writes_enabled: optional(nullable(boolean)),
  lark_message_effects_writes_enabled: optional(nullable(boolean)),
  outbound_messages_writes_enabled: optional(nullable(boolean)),
  native_media_writes_enabled: optional(nullable(boolean)),
  multi_consumer_writes_enabled: boolean,
  native_bot_mentions_writes_enabled: boolean,
  thread_contract_writes_enabled: boolean,
  platforms: array(RelayPlatformCapabilitySchema),
});
export type RelayAuthoringCapabilities = Output<typeof RelayAuthoringCapabilitiesSchema>;

/** Server tool metadata; does not grant channel, source or event permission. */
export const AgentToolCapabilitySchema = object({
  name: text,
  description: text,
  read_only: boolean,
  destructive: boolean,
  idempotent: boolean,
  open_world: boolean,
});
export type AgentToolCapability = Output<typeof AgentToolCapabilitySchema>;

export const CapabilityViewSchema = object({
  agent_tools: optional(nullable(array(AgentToolCapabilitySchema))),
  platforms: array(PlatformCapabilitySchema),
  source_features: optional(array(text)),
  relay_authoring: optional(nullable(RelayAuthoringCapabilitiesSchema)),
});
export type CapabilityView = Output<typeof CapabilityViewSchema>;

export const ConnectionViewSchema = object({
  connection_ref: text,
  status: StatusSchema,
  label: optional(nullable(text)),
  url: optional(nullable(text)),
  setup_url: optional(nullable(text)),
});
export type ConnectionView = Output<typeof ConnectionViewSchema>;

export const ConnectionPageSchema = object({
  items: array(ConnectionViewSchema),
  next_after: optional(nullable(text)),
});
export type ConnectionPage = Output<typeof ConnectionPageSchema>;

export const CreateConnectionSchema = object({
  record_id: text,
  event_ref: text,
  suggested_name: optional(nullable(text)),
});
export type CreateConnection = Output<typeof CreateConnectionSchema>;

export const ResourcePlatformSchema = enumeration('lark', 'feishu');
export type ResourcePlatform = Output<typeof ResourcePlatformSchema>;
export const ResourceConnectionSourceSchema = enumeration('personal', 'organization');
export type ResourceConnectionSource = Output<typeof ResourceConnectionSourceSchema>;
/** Nonsecret metadata; selection never grants a channel credential access. */
export const ResourceConnectionSchema = object({
  user_service_id: text,
  catalog_service_id: text,
  name: text,
  platform: ResourcePlatformSchema,
  source: ResourceConnectionSourceSchema,
});
export type ResourceConnection = Output<typeof ResourceConnectionSchema>;
export const ResourceConnectionListSchema = object({ items: array(ResourceConnectionSchema) });
export type ResourceConnectionList = Output<typeof ResourceConnectionListSchema>;
