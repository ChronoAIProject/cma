import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  text,
  type Output,
} from '../../schema.js';
import { ConformanceLevelSchema } from './primitives.js';
import {
  BindingProfileDriftSchema,
  DesiredProfileDriftSchema,
} from './channel-profile-revisions.js';

export const ConversationInspectionSchema = object({
  binding_id: text,
  generation: integer,
  incarnation: optional(integer),
  conversation_id: text,
  agent_id: text,
  state: text,
  agent_status: optional(nullable(text)),
  profile_drift: optional(nullable(BindingProfileDriftSchema)),
});
export type ConversationInspection = Output<typeof ConversationInspectionSchema>;

export const PartitionInspectionSchema = object({
  consumer_id: text,
  transport_conversation_id: text,
  mapping: ConversationInspectionSchema,
  assigned_profile_digest: optional(nullable(text)),
  desired_profile_drift: optional(nullable(DesiredProfileDriftSchema)),
});
export type PartitionInspection = Output<typeof PartitionInspectionSchema>;

export const ConversationResetSchema = object({
  binding_id: text,
  generation: positiveInteger,
  conversation_id: text,
  incarnation: positiveInteger,
  previous_agent_id: text,
  previous_conversation_id: text,
  successor_agent_id: text,
  state: enumeration('reserved'),
});
export type ConversationReset = Output<typeof ConversationResetSchema>;

export const PartitionResetRequestSchema = object({
  partition_key: text,
  expected_version: positiveInteger,
  expected_agent_id: text,
  consumer_id: optional(nullable(text)),
});
export type PartitionResetRequest = Output<typeof PartitionResetRequestSchema>;

export const PartitionResetResponseSchema = object({
  consumer_id: text,
  transport_conversation_id: text,
  reset: ConversationResetSchema,
});
export type PartitionResetResponse = Output<typeof PartitionResetResponseSchema>;

export const EndpointResponseSchema = object({
  native_lark_webhook_url: optional(nullable(text)),
  whatsapp_webhook_url: optional(nullable(text)),
  custom_event_url: text,
  nyxid_callback_url: text,
  telegram_webhook_url: optional(nullable(text)),
});
export type EndpointResponse = Output<typeof EndpointResponseSchema>;

export const FieldRefusalSchema = object({ field: text, reason: text });
export type FieldRefusal = Output<typeof FieldRefusalSchema>;

export const RekeyResponseSchema = object({ kid: integer });
export type RekeyResponse = Output<typeof RekeyResponseSchema>;

export const TestChannelResponseSchema = object({
  accepted_after_ms: integer,
  first_text: text,
  level: ConformanceLevelSchema,
});
export type TestChannelResponse = Output<typeof TestChannelResponseSchema>;

export const NativeDiscordConnectionStageSchema = enumeration(
  'unobserved',
  'resolve',
  'discovery',
  'lease',
  'authorize',
  'socket',
  'hello',
  'identify',
  'resume',
  'ready',
  'renew',
  'event_commit',
  'stopped',
);
export type NativeDiscordConnectionStage = Output<typeof NativeDiscordConnectionStageSchema>;
export const NativeDiscordConnectionOutcomeSchema = enumeration(
  'unknown',
  'pending',
  'ok',
  'retrying',
  'blocked',
  'stopped',
);
export type NativeDiscordConnectionOutcome = Output<typeof NativeDiscordConnectionOutcomeSchema>;
export const NativeDiscordConnectionFailureCodeSchema = enumeration(
  'unavailable',
  'authority_refused',
  'invalid_configuration',
  'missing_message_content_intent',
  'rate_limited',
  'protocol',
  'discovery_unauthorized',
  'bot_identity_mismatch',
  'sharding_unsupported',
  'session_limit_invalid',
  'lease_contended',
  'lease_owner_mismatch',
  'lease_source_mismatch',
  'lease_invalid',
  'identify_budget_exhausted',
  'identify_bucket_wait',
  'hello_timeout',
  'hello_invalid',
  'authentication_failed',
  'disallowed_intents',
  'invalid_intents',
  'sharding_required',
  'unsupported_version',
  'backpressure',
  'commit_unavailable',
  'heartbeat_lost',
  'renew_failed',
  'renew_timeout',
  'authority_revoked',
  'invalid_session',
  'reconnect',
  'transport',
);
export type NativeDiscordConnectionFailureCode = Output<
  typeof NativeDiscordConnectionFailureCodeSchema
>;

/** Local connection observations; absence/staleness does not authorize relay takeover. */
export const NativeDiscordConnectionStatusSchema = object({
  scope: text,
  stage: NativeDiscordConnectionStageSchema,
  outcome: NativeDiscordConnectionOutcomeSchema,
  connected: optional(nullable(boolean)),
  authenticated: optional(nullable(boolean)),
  ready: optional(nullable(boolean)),
  lease: text,
  identify: text,
  observed_at: optional(nullable(text)),
  valid_until: optional(nullable(text)),
  stale: boolean,
  last_ready_at: optional(nullable(text)),
  last_event_at: optional(nullable(text)),
  last_event_committed_at: optional(nullable(text)),
  last_dispatch_at: optional(nullable(text)),
  last_dispatch_outcome: text,
  last_failure_stage: optional(nullable(NativeDiscordConnectionStageSchema)),
  last_failure_code: optional(nullable(NativeDiscordConnectionFailureCodeSchema)),
  last_failure_at: optional(nullable(text)),
  retry_at: optional(nullable(text)),
});
export type NativeDiscordConnectionStatus = Output<typeof NativeDiscordConnectionStatusSchema>;

/** Native intake eligibility; selection is not a socket-health claim. */
export const NativeDiscordActivationSchema = object({
  state: text,
  detail: text,
  capacity: integer,
  selected_records: integer,
  connection: optional(nullable(NativeDiscordConnectionStatusSchema)),
});
export type NativeDiscordActivation = Output<typeof NativeDiscordActivationSchema>;
