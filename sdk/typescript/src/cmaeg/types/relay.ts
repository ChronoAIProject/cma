import { RefusalSchema, SecretSchema } from './primitives.js';
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

export const RelayAgentSchema = object({ api_key_id: text, name: text });
export type RelayAgent = Output<typeof RelayAgentSchema>;

export const RelayAttachmentSchema = object({
  content_type: text,
  download_url: optional(nullable(text)),
  file_key: optional(nullable(text)),
  filename: optional(nullable(text)),
  image_key: optional(nullable(text)),
  mime_type: optional(nullable(text)),
  platform_message_id: optional(nullable(text)),
  size_bytes: optional(nullable(integer)),
  url: text,
});
export type RelayAttachment = Output<typeof RelayAttachmentSchema>;

export const RelayContentSchema = object({
  attachments: optional(array(RelayAttachmentSchema)),
  text: optional(nullable(text)),
  type: text,
});
export type RelayContent = Output<typeof RelayContentSchema>;

export const RelayConversationSchema = object({ id: text, platform_id: text, type: text });
export type RelayConversation = Output<typeof RelayConversationSchema>;

export const RelaySenderSchema = object({
  display_name: optional(nullable(text)),
  platform_id: text,
});
export type RelaySender = Output<typeof RelaySenderSchema>;

export const CallbackPayloadSchema = object({
  agent: RelayAgentSchema,
  content: RelayContentSchema,
  conversation: RelayConversationSchema,
  correlation_id: text,
  message_id: text,
  platform: text,
  raw_platform_data: optional(anything),
  reply_to_message_id: optional(nullable(text)),
  reply_to_platform_message_id: optional(nullable(text)),
  reply_token: SecretSchema,
  sender: RelaySenderSchema,
  thread_id: optional(nullable(SecretSchema)),
  timestamp: text,
});
export type CallbackPayload = Output<typeof CallbackPayloadSchema>;

export const CallbackResponseSchema = tagged(
  {
    accepted: object({ conversation_id: text, event_id: text, status: enumeration('accepted') }),
    refused: object({ reason: RefusalSchema, status: enumeration('refused') }),
  },
  'status',
);
export type CallbackResponse = Output<typeof CallbackResponseSchema>;
