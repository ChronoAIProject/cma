import { array, enumeration, nullable, object, optional, text, type Output } from '../../schema.js';

export const StatusSchema = enumeration(
  'draft',
  'awaiting_platform',
  'ready',
  'cancelled',
  'expired',
);
export type Status = Output<typeof StatusSchema>;

export const PlatformCapabilitySchema = object({
  id: text,
  label: text,
  connection_methods: array(text),
  optional_capabilities: array(text),
});
export type PlatformCapability = Output<typeof PlatformCapabilitySchema>;

export const CapabilityViewSchema = object({ platforms: array(PlatformCapabilitySchema) });
export type CapabilityView = Output<typeof CapabilityViewSchema>;

export const ConnectionViewSchema = object({
  connection_ref: text,
  status: StatusSchema,
  label: optional(nullable(text)),
  url: optional(nullable(text)),
  setup_url: optional(nullable(text)),
});
export type ConnectionView = Output<typeof ConnectionViewSchema>;

export const ConnectionPageSchema = object({
  items: array(ConnectionViewSchema),
  next_after: optional(nullable(text)),
});
export type ConnectionPage = Output<typeof ConnectionPageSchema>;

export const CreateConnectionSchema = object({
  record_id: text,
  event_ref: text,
  suggested_name: optional(nullable(text)),
});
export type CreateConnection = Output<typeof CreateConnectionSchema>;
