import { boolean, object, optional, text, type Output } from '../../schema.js';

export const LarkResourceBindingSchema = object({
  user_service_id: text,
  catalog_service_id: text,
  app_id: text,
});
export type LarkResourceBinding = Output<typeof LarkResourceBindingSchema>;

export const LarkMessageEffectsBindingSchema = object({
  connection: LarkResourceBindingSchema,
  delete: optional(boolean),
  reaction: optional(boolean),
});
export type LarkMessageEffectsBinding = Output<typeof LarkMessageEffectsBindingSchema>;
