import { EventWorkerProfileSchema, NyxIdIdentitySchema } from './profile.js';
import {
  BindingIdSchema,
  ChannelIdSchema,
  ConformanceLevelSchema,
  GroupAdmissionSchema,
  PartitionKeySchema,
  PlatformSchema,
  ProviderSlugSchema,
  RecordIdSchema,
  SecretSchema,
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

export const IdentityAdmissionSchema = tagged(
  {
    deny: object({ type: enumeration('deny') }),
    open: object({ type: enumeration('open') }),
    allowlist: object({ ids: array(text), type: enumeration('allowlist') }),
  },
  'type',
);
export type IdentityAdmission = Output<typeof IdentityAdmissionSchema>;

export const AdmissionPolicySchema = tagged(
  {
    all: object({ type: enumeration('all') }),
    allowlist: object({ sender_ids: array(text), type: enumeration('allowlist') }),
    scoped: object({
      chats: IdentityAdmissionSchema,
      groups: GroupAdmissionSchema,
      senders: IdentityAdmissionSchema,
      type: enumeration('scoped'),
    }),
  },
  'type',
);
export type AdmissionPolicy = Output<typeof AdmissionPolicySchema>;

export const HmacSecretRequestSchema = object({ key_id: text, secret: SecretSchema });
export type HmacSecretRequest = Output<typeof HmacSecretRequestSchema>;

export const CredentialsRequestSchema = object({
  agent_key: SecretSchema,
  channel_key: optional(nullable(SecretSchema)),
  telegram_bot_token: optional(nullable(SecretSchema)),
  telegram_webhook_secret: optional(nullable(SecretSchema)),
  hmac_secrets: optional(array(HmacSecretRequestSchema)),
});
export type CredentialsRequest = Output<typeof CredentialsRequestSchema>;

export const PartitionRuleSchema = object({ key: PartitionKeySchema, version: integer });
export type PartitionRule = Output<typeof PartitionRuleSchema>;

export const ProviderTargetSchema = tagged(
  {
    profile: object({ type: enumeration('profile') }),
    existing_agent: object({ type: enumeration('existing_agent'), agent_id: text }),
  },
  'type',
);
export type ProviderTarget = Output<typeof ProviderTargetSchema>;

export const ProviderRequestSchema = object({
  slug: ProviderSlugSchema,
  target: optional(ProviderTargetSchema),
});
export type ProviderRequest = Output<typeof ProviderRequestSchema>;

export const SinkSchema = tagged(
  {
    source: object({ type: enumeration('source') }),
    result_webhook: object({ hmac_key_id: text, type: enumeration('result_webhook'), url: text }),
    sync: object({ type: enumeration('sync') }),
  },
  'type',
);
export type Sink = Output<typeof SinkSchema>;

export const ReplyPolicySchema = object({
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

export const SourceRequestSchema = tagged(
  {
    telegram: object({
      admission: AdmissionPolicySchema,
      bot_id: text,
      bot_username: optional(nullable(text)),
      type: enumeration('telegram'),
    }),
    nyxid_relay: object({
      admission: AdmissionPolicySchema,
      bot_id: optional(nullable(text)),
      bot_username: optional(nullable(text)),
      issuer: text,
      key_id: text,
      platform: PlatformSchema,
      route_ids: array(text),
      trust_normalized: optional(boolean),
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

export const ProviderBindingSchema = object({
  binding_id: BindingIdSchema,
  level: ConformanceLevelSchema,
  slug: ProviderSlugSchema,
  target: optional(ProviderTargetSchema),
});
export type ProviderBinding = Output<typeof ProviderBindingSchema>;

export const PublicSecretsSchema = object({ agent_key_hash: text });
export type PublicSecrets = Output<typeof PublicSecretsSchema>;

export const SourceSchema = tagged(
  {
    telegram: object({
      admission: AdmissionPolicySchema,
      bot_id: text,
      bot_username: optional(nullable(text)),
      type: enumeration('telegram'),
    }),
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
  schema_version: integer,
  secrets: PublicSecretsSchema,
  sources: array(SourceSchema),
  version: integer,
});
export type PublicChannelDefinition = Output<typeof PublicChannelDefinitionSchema>;

export const EndpointResponseSchema = object({
  custom_event_url: text,
  nyxid_callback_url: text,
  telegram_webhook_url: optional(nullable(text)),
});
export type EndpointResponse = Output<typeof EndpointResponseSchema>;

export const FieldRefusalSchema = object({ field: text, reason: text });
export type FieldRefusal = Output<typeof FieldRefusalSchema>;

export const ProviderResponseSchema = object({
  binding_id: text,
  honoured: array(text),
  level: ConformanceLevelSchema,
  refused: array(FieldRefusalSchema),
  slug: text,
});
export type ProviderResponse = Output<typeof ProviderResponseSchema>;

export const ChannelResponseSchema = object({
  channel_id: text,
  definition: PublicChannelDefinitionSchema,
  endpoints: EndpointResponseSchema,
  provider: ProviderResponseSchema,
  record_id: text,
  version: integer,
});
export type ChannelResponse = Output<typeof ChannelResponseSchema>;

export const CredentialPatchSchema = object({
  agent_key: optional(nullable(SecretSchema)),
  channel_key: optional(nullable(SecretSchema)),
  telegram_bot_token: optional(nullable(SecretSchema)),
  telegram_webhook_secret: optional(nullable(SecretSchema)),
  hmac_secrets: optional(nullable(array(HmacSecretRequestSchema))),
});
export type CredentialPatch = Output<typeof CredentialPatchSchema>;

export const ChannelUpdateRequestSchema = object({
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

export const RekeyResponseSchema = object({ kid: integer });
export type RekeyResponse = Output<typeof RekeyResponseSchema>;

export const TestChannelResponseSchema = object({
  accepted_after_ms: integer,
  first_text: text,
  level: ConformanceLevelSchema,
});
export type TestChannelResponse = Output<typeof TestChannelResponseSchema>;
