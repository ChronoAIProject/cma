import { nullable, object, optional, text, type Output } from '../../schema.js';

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
