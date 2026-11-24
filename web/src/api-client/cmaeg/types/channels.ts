import { ProviderBindingSchema, ProviderRequestSchema } from './provider-binding.js';
export * from './provider-binding.js';
import {
  EndpointResponseSchema,
  FieldRefusalSchema,
  NativeDiscordActivationSchema,
} from './channel-results.js';
export * from './channel-results.js';
import { NativeLarkIdentitySchema } from './primitives.js';
import { CredentialsRequestSchema, CredentialPatchSchema } from './credentials.js';
export * from './credentials.js';
import { LarkResourceBindingSchema, LarkMessageEffectsBindingSchema } from './lark.js';
export * from './lark.js';
import { EventWorkerProfileSchema, NyxIdIdentitySchema } from './profile.js';
import {
  ChannelIdSchema,
  ConformanceLevelSchema,
  GroupAdmissionSchema,
  PartitionKeySchema,
  PlatformSchema,
  RecordIdSchema,
} from './primitives.js';
import {
  array,
  boolean,
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
import { GitHubSourceSchema } from './github.js';
export const IdentityAdmissionSchema = tagged(
  {
    deny: object({ type: enumeration('deny') }),
    open: object({ type: enumeration('open') }),
    allowlist: object({ ids: array(text), type: enumeration('allowlist') }),
  },
  'type',
);
export type IdentityAdmission = Output<typeof IdentityAdmissionSchema>;
export const BotMessageAdmissionSchema = enumeration('deny', 'native_mentions');
export type BotMessageAdmission = Output<typeof BotMessageAdmissionSchema>;
export const AdmissionPolicySchema = tagged(
  {
    all: object({ type: enumeration('all') }),
    allowlist: object({ sender_ids: array(text), type: enumeration('allowlist') }),
    scoped: object({
      bot_messages: optional(BotMessageAdmissionSchema),
      chats: IdentityAdmissionSchema,
      groups: GroupAdmissionSchema,
      senders: IdentityAdmissionSchema,
      type: enumeration('scoped'),
    }),
  },
  'type',
);
export type AdmissionPolicy = Output<typeof AdmissionPolicySchema>;

export const NativeLarkSourceSchema = object({
  conversation_history: optional(boolean),
  separate_app: boolean,
  identity: NativeLarkIdentitySchema,
  admission: AdmissionPolicySchema,
});
export type NativeLarkSource = Output<typeof NativeLarkSourceSchema>;

export const PartitionRuleSchema = object({
  key: PartitionKeySchema,
  custom_key: optional(nullable(PartitionKeySchema)),
  version: integer,
});
export type PartitionRule = Output<typeof PartitionRuleSchema>;

export const SinkSchema = tagged(
  {
    source: object({ type: enumeration('source') }),
    result_webhook: object({ hmac_key_id: text, type: enumeration('result_webhook'), url: text }),
    sync: object({ type: enumeration('sync') }),
  },
  'type',
);
export type Sink = Output<typeof SinkSchema>;

export const ThreadContractSchema = object({
  version: integer,
  follow_chat_ids: optional(array(text)),
});
export type ThreadContract = Output<typeof ThreadContractSchema>;

export const ReplyPolicySchema = object({
  thread_contract: optional(nullable(ThreadContractSchema)),
  ack_after_ms: integer,
  auto_final: boolean,
  sinks: optional(array(SinkSchema)),
});
export type ReplyPolicy = Output<typeof ReplyPolicySchema>;

export const CustomAuthSchema = tagged(
  {
    hmac: object({ key_id: text, type: enumeration('hmac') }),
    nyxid_key: object({ key_hashes: array(text), type: enumeration('nyxid_key') }),
  },
  'type',
);
export type CustomAuth = Output<typeof CustomAuthSchema>;

export const CustomAttachmentsSchema = object({
  origins: optional(array(text)),
  pointer: optional(text),
});
export type CustomAttachments = Output<typeof CustomAttachmentsSchema>;

const telegramSourceFields = {
  topic_lifecycle: optional(boolean),
  topic_creation: optional(boolean),
  message_events: optional(boolean),
  channel_posts: optional(boolean),
  admission: AdmissionPolicySchema,
  bot_id: text,
  bot_username: optional(nullable(text)),
  type: enumeration('telegram'),
};
const TelegramSourceSchema = object({
  ...telegramSourceFields,
  member_join_chat_ids: optional(array(text)),
});
const TelegramSourceRequestSchema = object({
  ...telegramSourceFields,
  member_join_chat_ids: optional(nullable(array(text))),
});

export const SourceRequestSchema = tagged(
  {
    github: GitHubSourceSchema,
    native_lark: object({
      type: enumeration('native_lark'),
      conversation_history: optional(boolean),
      separate_app: boolean,
      identity: NativeLarkIdentitySchema,
      admission: AdmissionPolicySchema,
    }),
    whatsapp: object({
      app_id: text,
      waba_id: text,
      phone_number_id: text,
      forms_enabled: optional(boolean),
      message_events: optional(boolean),
      group_ids: optional(array(text)),
      admission: AdmissionPolicySchema,
      type: enumeration('whatsapp'),
    }),
    discord: object({
      admission: AdmissionPolicySchema,
      bot_id: text,
      intents: integer,
      thread_history: optional(boolean),
      thread_lifecycle: optional(boolean),
      conversation_history: optional(boolean),
      type: enumeration('discord'),
    }),
    telegram: TelegramSourceRequestSchema,
    nyxid_relay: object({
      admission: AdmissionPolicySchema,
      bot_id: optional(nullable(text)),
      bot_username: optional(nullable(text)),
      issuer: text,
      key_id: text,
      platform: PlatformSchema,
      route_ids: array(text),
      trust_normalized: optional(boolean),
      verify_raw_event: optional(boolean),
      lark_resources: optional(nullable(LarkResourceBindingSchema)),
      lark_message_effects: optional(nullable(LarkMessageEffectsBindingSchema)),
      member_join_chat_ids: optional(nullable(array(text))),
      type: enumeration('nyxid_relay'),
    }),
    custom: object({
      attachments: optional(nullable(CustomAttachmentsSchema)),
      auth: CustomAuthSchema,
      prompt_template: optional(nullable(text)),
      schema: optional(anything),
      type: enumeration('custom'),
    }),
  },
  'type',
);
export type SourceRequest = Output<typeof SourceRequestSchema>;
export const ChannelCreateRequestSchema = object({
  durable_custom_execution: optional(boolean),
  connection_ref: optional(nullable(text)),
  connection_admission: optional(nullable(AdmissionPolicySchema)),
  credentials: CredentialsRequestSchema,
  name: text,
  partition: PartitionRuleSchema,
  profile: EventWorkerProfileSchema,
  provider: ProviderRequestSchema,
  record_id: optional(nullable(RecordIdSchema)),
  reply: ReplyPolicySchema,
  schema_version: integer,
  sources: array(SourceRequestSchema),
});
export type ChannelCreateRequest = Output<typeof ChannelCreateRequestSchema>;

export const ChannelCreationSchema = object({
  channel_id: ChannelIdSchema,
  fingerprint: text,
  ready: boolean,
});
export type ChannelCreation = Output<typeof ChannelCreationSchema>;

/** A channel consumer's id: `default` or `csm_` and 12 lowercase hex digits. */
export const ConsumerIdSchema = text;
export type ConsumerId = Output<typeof ConsumerIdSchema>;

/** A consumer's chat name, `^[a-z][a-z0-9_-]{1,31}$`, unique per channel. */
export const ConsumerNameSchema = text;
export type ConsumerName = Output<typeof ConsumerNameSchema>;

/** A routing rule's id, `^[a-z0-9_-]{1,32}$`, unique per channel. */
export const RuleIdSchema = text;
export type RuleId = Output<typeof RuleIdSchema>;

/** A source slot: `telegram`, `relay_<platform>`, `custom` or `agent`. */
export const SourceIdSchema = text;
export type SourceId = Output<typeof SourceIdSchema>;

/** An exact match, or a prefix match when it ends in one `*`. */
export const PatternSchema = text;
export type Pattern = Output<typeof PatternSchema>;

export const ActivityKindNameSchema = enumeration('message', 'command', 'interaction', 'event');
export type ActivityKindName = Output<typeof ActivityKindNameSchema>;

export const ReplyChoiceSchema = enumeration('origin', 'default');
export type ReplyChoice = Output<typeof ReplyChoiceSchema>;

export const ReplyLabelSchema = enumeration('auto', 'always', 'never');
export type ReplyLabel = Output<typeof ReplyLabelSchema>;

export const ConsumerReplySchema = object({
  auto_final: optional(nullable(boolean)),
  destination: optional(ReplyChoiceSchema),
});
export type ConsumerReply = Output<typeof ConsumerReplySchema>;

export const PublishPolicySchema = object({
  enabled: boolean,
  targets: optional(array(ConsumerIdSchema)),
});
export type PublishPolicy = Output<typeof PublishPolicySchema>;

/** An additional consumer beside `default`; it exposes only its key's public hash. */
export const ConsumerSchema = object({
  agent_key_hash: text,
  created_at: text,
  id: ConsumerIdSchema,
  label: optional(nullable(text)),
  mentionable: optional(boolean),
  name: ConsumerNameSchema,
  non_human: optional(nullable(boolean)),
  pending: optional(boolean),
  profile: EventWorkerProfileSchema,
  profile_digest: text,
  provider: ProviderBindingSchema,
  publish: optional(PublishPolicySchema),
  reply: optional(ConsumerReplySchema),
});
export type Consumer = Output<typeof ConsumerSchema>;

/** How the implicit `default` consumer is presented. */
export const DefaultConsumerSchema = object({
  label: optional(nullable(text)),
  mentionable: optional(boolean),
  name: ConsumerNameSchema,
  non_human: optional(nullable(boolean)),
  publish: optional(PublishPolicySchema),
  reply: optional(ConsumerReplySchema),
});
export type DefaultConsumer = Output<typeof DefaultConsumerSchema>;

export const RuleMatchSchema = object({
  chats: optional(array(text)),
  event_sources: optional(array(PatternSchema)),
  event_types: optional(array(PatternSchema)),
  kinds: optional(array(ActivityKindNameSchema)),
  senders: optional(array(text)),
  sources: optional(array(SourceIdSchema)),
});
export type RuleMatch = Output<typeof RuleMatchSchema>;

export const RoutingRuleSchema = object({
  id: RuleIdSchema,
  targets: array(ConsumerIdSchema),
  when: optional(RuleMatchSchema),
});
export type RoutingRule = Output<typeof RoutingRuleSchema>;

/** Gateway routing directives; an absent switch is on. */
export const AddressingSchema = object({
  command: optional(boolean),
  event_field: optional(boolean),
  mentions: optional(boolean),
  unknown_target_notice: optional(boolean),
});
export type Addressing = Output<typeof AddressingSchema>;

export const DefaultReplySchema = tagged(
  {
    telegram_chat: object({
      chat_id: text,
      source_id: SourceIdSchema,
      thread_id: optional(nullable(text)),
      type: enumeration('telegram_chat'),
    }),
    relay_conversation: object({
      route_id: text,
      source_id: SourceIdSchema,
      type: enumeration('relay_conversation'),
    }),
    sinks: object({ type: enumeration('sinks') }),
  },
  'type',
);
export type DefaultReply = Output<typeof DefaultReplySchema>;

export const AgentEventPolicySchema = object({
  max_hops: optional(integer),
  per_channel_per_minute: optional(integer),
  per_consumer_per_minute: optional(integer),
});
export type AgentEventPolicy = Output<typeof AgentEventPolicySchema>;

/** Multi-consumer routing, exposed read-only on a channel definition. */
export const RoutingPolicySchema = object({
  addressing: optional(AddressingSchema),
  agent_events: optional(AgentEventPolicySchema),
  consumers: optional(array(ConsumerSchema)),
  default_consumer: DefaultConsumerSchema,
  default_reply: optional(nullable(DefaultReplySchema)),
  default_targets: array(ConsumerIdSchema),
  max_targets: optional(integer),
  reply_label: optional(ReplyLabelSchema),
  rules: optional(array(RoutingRuleSchema)),
  version: integer,
});
export type RoutingPolicy = Output<typeof RoutingPolicySchema>;

export const PublicSecretsSchema = object({ agent_key_hash: text });
export type PublicSecrets = Output<typeof PublicSecretsSchema>;
export const SourceSchema = tagged(
  {
    github: GitHubSourceSchema,
    native_lark: object({
      type: enumeration('native_lark'),
      conversation_history: optional(boolean),
      separate_app: boolean,
      identity: NativeLarkIdentitySchema,
      admission: AdmissionPolicySchema,
    }),
    whatsapp: object({
      app_id: text,
      waba_id: text,
      phone_number_id: text,
      forms_enabled: optional(boolean),
      message_events: optional(boolean),
      group_ids: optional(array(text)),
      admission: AdmissionPolicySchema,
      type: enumeration('whatsapp'),
    }),
    discord: object({
      admission: AdmissionPolicySchema,
      bot_id: text,
      intents: integer,
      thread_history: optional(boolean),
      thread_lifecycle: optional(boolean),
      conversation_history: optional(boolean),
      type: enumeration('discord'),
    }),
    telegram: TelegramSourceSchema,
    nyxid_relay: object({
      admission: AdmissionPolicySchema,
      bot_id: optional(nullable(text)),
      bot_username: optional(nullable(text)),
      issuer: text,
      jwks_url: text,
      key_id: text,
      platform: PlatformSchema,
      route_ids: array(text),
      trust_normalized: optional(boolean),
      verify_raw_event: optional(boolean),
      lark_resources: optional(nullable(LarkResourceBindingSchema)),
      lark_message_effects: optional(nullable(LarkMessageEffectsBindingSchema)),
      member_join_chat_ids: optional(array(text)),
      type: enumeration('nyxid_relay'),
    }),
    custom: object({
      attachments: optional(nullable(CustomAttachmentsSchema)),
      auth: CustomAuthSchema,
      prompt_template: optional(nullable(text)),
      schema: optional(anything),
      type: enumeration('custom'),
    }),
  },
  'type',
);
export type Source = Output<typeof SourceSchema>;
export const PublicChannelDefinitionSchema = object({
  durable_custom_execution: optional(boolean),
  connection_ref: optional(nullable(text)),
  created_at: text,
  creation: optional(nullable(ChannelCreationSchema)),
  name: optional(text),
  owner: NyxIdIdentitySchema,
  partition: PartitionRuleSchema,
  profile: EventWorkerProfileSchema,
  profile_digest: text,
  provider: ProviderBindingSchema,
  record_id: RecordIdSchema,
  reply: ReplyPolicySchema,
  routing: optional(nullable(RoutingPolicySchema)),
  schema_version: integer,
  secrets: PublicSecretsSchema,
  sources: array(SourceSchema),
  version: integer,
});
export type PublicChannelDefinition = Output<typeof PublicChannelDefinitionSchema>;

export const ProviderResponseSchema = object({
  binding_id: text,
  honoured: array(text),
  level: ConformanceLevelSchema,
  refused: array(FieldRefusalSchema),
  slug: text,
});
export type ProviderResponse = Output<typeof ProviderResponseSchema>;

export const ChannelResponseSchema = object({
  native_discord_activation: optional(nullable(NativeDiscordActivationSchema)),
  thread_contract_versions: optional(array(integer)),
  channel_id: text,
  definition: PublicChannelDefinitionSchema,
  endpoints: EndpointResponseSchema,
  provider: ProviderResponseSchema,
  record_id: text,
  version: integer,
});
export type ChannelResponse = Output<typeof ChannelResponseSchema>;

/** Owned channel metadata; legacy records omit an unrecoverable original ID and endpoints. */
export const ChannelListItemSchema = object({
  native_discord_activation: optional(nullable(NativeDiscordActivationSchema)),
  channel_id: optional(nullable(text)),
  definition: PublicChannelDefinitionSchema,
  endpoints: optional(nullable(EndpointResponseSchema)),
  provider: ProviderResponseSchema,
  record_id: text,
  version: integer,
});
export type ChannelListItem = Output<typeof ChannelListItemSchema>;

export const ChannelPageSchema = object({
  items: array(ChannelListItemSchema),
  next_cursor: optional(nullable(text)),
});
export type ChannelPage = Output<typeof ChannelPageSchema>;

export const ChannelUpdateRequestSchema = object({
  durable_custom_execution: optional(nullable(boolean)),
  connection_ref: optional(nullable(text)),
  connection_admission: optional(nullable(AdmissionPolicySchema)),
  credentials: optional(nullable(CredentialPatchSchema)),
  expected_version: integer,
  name: text,
  partition: PartitionRuleSchema,
  profile: EventWorkerProfileSchema,
  provider: ProviderRequestSchema,
  reply: ReplyPolicySchema,
  schema_version: integer,
  sources: array(SourceRequestSchema),
});
export type ChannelUpdateRequest = Output<typeof ChannelUpdateRequestSchema>;
