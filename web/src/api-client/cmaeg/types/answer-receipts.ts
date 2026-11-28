import { enumeration, integer, object, text, type Output } from '../../schema.js';
import { OutboundMessageSchema } from './messages.js';
import { BindingIdSchema, ChannelIdSchema, EventIdSchema } from './primitives.js';

export const AcceptedAnswerRequestSchema = object({
  response_id: text,
  item_id: text,
  message: OutboundMessageSchema,
});
export type AcceptedAnswerRequest = Output<typeof AcceptedAnswerRequestSchema>;

export const AnswerReceiptLookupSchema = object({
  channel_id: ChannelIdSchema,
  binding_id: BindingIdSchema,
  event_id: EventIdSchema,
  response_id: text,
  item_id: text,
});
export type AnswerReceiptLookup = Output<typeof AnswerReceiptLookupSchema>;

export const AnswerDispatchSchema = enumeration('not_dispatched', 'attempted', 'indeterminate');
export type AnswerDispatch = Output<typeof AnswerDispatchSchema>;

export const AnswerOutcomeSchema = enumeration('confirmed', 'rejected', 'unknown');
export type AnswerOutcome = Output<typeof AnswerOutcomeSchema>;

export const AnswerReasonSchema = enumeration(
  'platform_confirmed',
  'refused_before_dispatch',
  'platform_refused',
  'partial_delivery',
  'delivery_unconfirmed',
);
export type AnswerReason = Output<typeof AnswerReasonSchema>;

export const AnswerReceiptResultSchema = object({
  delivery_id: text,
  settled_at: integer,
  dispatch: AnswerDispatchSchema,
  outcome: AnswerOutcomeSchema,
  reason: AnswerReasonSchema,
});
export type AnswerReceiptResult = Output<typeof AnswerReceiptResultSchema>;

export const AnswerReceiptSchema = object({
  answer: AnswerReceiptLookupSchema,
  result: AnswerReceiptResultSchema,
});
export type AnswerReceipt = Output<typeof AnswerReceiptSchema>;
