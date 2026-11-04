import { integer, nullable, object, optional, text, type Output } from '../../schema.js';
import { ConformanceLevelSchema } from './primitives.js';

export const ConversationInspectionSchema = object({
  binding_id: text,
  generation: integer,
  conversation_id: text,
  agent_id: text,
  state: text,
  agent_status: optional(nullable(text)),
});
export type ConversationInspection = Output<typeof ConversationInspectionSchema>;

export const PartitionInspectionSchema = object({
  consumer_id: text,
  transport_conversation_id: text,
  mapping: ConversationInspectionSchema,
});
export type PartitionInspection = Output<typeof PartitionInspectionSchema>;

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

/** Native intake eligibility; selection is not a socket-health claim. */
export const NativeDiscordActivationSchema = object({
  state: text,
  detail: text,
  capacity: integer,
  selected_records: integer,
});
export type NativeDiscordActivation = Output<typeof NativeDiscordActivationSchema>;
