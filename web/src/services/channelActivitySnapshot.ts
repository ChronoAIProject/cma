import {
  ActivityRecipientSchema,
  ConsumerStateSchema,
  QuotedContextSummarySchema,
} from '../api-client/cmaeg/types/channel-activity';
import {
  array,
  boolean,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../api-client/schema';

const optionalText = optional(nullable(text));

const delivery = object({
  step: text,
  destination: text,
  outcome: text,
  observed_at: text,
  confirmed_at: optionalText,
  platform_message_id: optionalText,
  recipients: optional(nullable(array(ActivityRecipientSchema))),
});

const consumer = object({
  consumer_id: text,
  display_name: optionalText,
  agent_id: optionalText,
  provider: optionalText,
  conversation_id: optionalText,
  execution_reference: optionalText,
  refusal_code: optionalText,
  last_non_admission_code: optionalText,
  state: ConsumerStateSchema,
  dispatched_at: optionalText,
  accepted_at: optionalText,
  produced_at: optionalText,
  produced_by: optionalText,
  completed_at: optionalText,
  response_id: optionalText,
  deliveries: array(delivery),
});

const event = object({
  event_id: text,
  source_refusal: optionalText,
  quoted_context: optional(nullable(QuotedContextSummarySchema)),
  kind: optionalText,
  event_type: optionalText,
  source_occurred_at: optionalText,
  // Historical feed receipts can omit routing metadata required by the Gateway client.
  source: object({
    type: text,
    platform: optionalText,
    bot_id: optionalText,
    route_id: optionalText,
  }),
  actor: object({ id: text, kind: optionalText, display_name: optionalText }),
  conversation: object({ id: text, kind: text, thread_id: optionalText }),
  occurred_at: text,
  received_at: text,
  consumers: array(consumer),
});

export const parseActivitySnapshot = object({
  items: array(event),
  cursor: text,
  retention_days: integer,
  limit: integer,
  has_more: boolean,
});

export type ActivityDelivery = Output<typeof delivery>;
export type ActivityConsumer = Output<typeof consumer>;
export type ChannelActivityEvent = Output<typeof event>;
export type ActivityPage = Output<typeof parseActivitySnapshot>;
