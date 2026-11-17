import {
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../../schema.js';
import { SourceIdSchema } from './channels.js';
import { DeliveryOutcomeSchema, OutboundMessageSchema, RenderingReportSchema } from './messages.js';

export const PrepareOutboundMessageSchema = object({
  source_id: SourceIdSchema,
  route_id: text,
  message: OutboundMessageSchema,
});
export type PrepareOutboundMessage = Output<typeof PrepareOutboundMessageSchema>;
export const OutboundStatusSchema = enumeration('prepared', 'committed', 'cancelled');
export type OutboundStatus = Output<typeof OutboundStatusSchema>;
export const OutboundReceiptSchema = object({
  delivery_id: text,
  source_id: SourceIdSchema,
  route_id: text,
  status: OutboundStatusSchema,
  expires_at: integer,
  retained_until: integer,
  rendering: RenderingReportSchema,
  delivery: optional(nullable(DeliveryOutcomeSchema)),
});
export type OutboundReceipt = Output<typeof OutboundReceiptSchema>;
