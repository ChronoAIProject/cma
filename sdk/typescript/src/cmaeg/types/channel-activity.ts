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
import {
  QuoteRelationSchema,
  QuotedContentKindSchema,
  ActorSchema,
  ActivityConversationSchema,
  SourceKindSchema,
} from './activities.js';

export const ConsumerStateSchema = enumeration(
  'received',
  'queued',
  'dispatching',
  'accepted',
  'produced',
  'completed',
  'refused',
  'failed',
  'unknown',
);
export type ConsumerState = Output<typeof ConsumerStateSchema>;

export const ActivityRecipientSchema = object({
  kind: text,
  id: text,
  thread_id: optional(nullable(text)),
});
export type ActivityRecipient = Output<typeof ActivityRecipientSchema>;

export const ActivityDeliverySchema = object({
  step: text,
  destination: text,
  outcome: text,
  observed_at: text,
  confirmed_at: optional(nullable(text)),
  platform_message_id: optional(nullable(text)),
  recipients: array(ActivityRecipientSchema),
});
export type ActivityDelivery = Output<typeof ActivityDeliverySchema>;

export const ActivityConsumerSchema = object({
  consumer_id: text,
  display_name: optional(nullable(text)),
  agent_id: optional(nullable(text)),
  provider: optional(nullable(text)),
  conversation_id: optional(nullable(text)),
  state: ConsumerStateSchema,
  dispatched_at: optional(nullable(text)),
  accepted_at: optional(nullable(text)),
  produced_at: optional(nullable(text)),
  completed_at: optional(nullable(text)),
  response_id: optional(nullable(text)),
  deliveries: array(ActivityDeliverySchema),
});
export type ActivityConsumer = Output<typeof ActivityConsumerSchema>;

/** Availability metadata only; never quoted text or parent IDs. */
export const QuotedContextSummarySchema = object({
  relation: QuoteRelationSchema,
  text_available: boolean,
  text_truncated: boolean,
  content_kinds: array(QuotedContentKindSchema),
});
export type QuotedContextSummary = Output<typeof QuotedContextSummarySchema>;

export const ChannelActivitySchema = object({
  quoted_context: optional(nullable(QuotedContextSummarySchema)),
  event_id: text,
  /** Payload category only; absent on historical receipts. */
  kind: optional(nullable(text)),
  /** CloudEvent type; event data is never included. */
  event_type: optional(nullable(text)),
  /** Explicit CloudEvent time, absent when the source did not report one. */
  source_occurred_at: optional(nullable(text)),
  source: SourceKindSchema,
  actor: ActorSchema,
  conversation: ActivityConversationSchema,
  /** Normalized event time: source timestamp when supplied, otherwise gateway normalization time. */
  occurred_at: text,
  received_at: text,
  consumers: array(ActivityConsumerSchema),
});
export type ChannelActivity = Output<typeof ChannelActivitySchema>;

/** Complete bounded snapshot; replace prior items atomically. */
export const ActivityPageSchema = object({
  items: array(ChannelActivitySchema),
  cursor: text,
  retention_days: integer,
  limit: integer,
  has_more: boolean,
});
export type ActivityPage = Output<typeof ActivityPageSchema>;
