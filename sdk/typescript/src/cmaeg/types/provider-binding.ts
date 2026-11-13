import { BindingIdSchema, ConformanceLevelSchema, ProviderSlugSchema } from './primitives.js';
import { array, enumeration, object, optional, tagged, text, type Output } from '../../schema.js';

export const ProviderTargetSchema = tagged(
  {
    profile: object({ type: enumeration('profile') }),
    existing_agent: object({ type: enumeration('existing_agent'), agent_id: text }),
  },
  'type',
);
export type ProviderTarget = Output<typeof ProviderTargetSchema>;

export const ProviderRequestSchema = object({
  slug: ProviderSlugSchema,
  target: optional(ProviderTargetSchema),
});
export type ProviderRequest = Output<typeof ProviderRequestSchema>;

export const ProviderBindingSchema = object({
  capabilities: optional(array(text)),
  binding_id: BindingIdSchema,
  level: ConformanceLevelSchema,
  slug: ProviderSlugSchema,
  target: optional(ProviderTargetSchema),
});
export type ProviderBinding = Output<typeof ProviderBindingSchema>;
