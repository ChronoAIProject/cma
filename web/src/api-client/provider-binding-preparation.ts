import { object, positiveInteger, tagged, enumeration, text, type Output } from './schema.js';

/** Immutable manager-authenticated admission returned by preparation inspection. */
export const BindingPreparationIntentSchema = object({
  version: positiveInteger,
  admission_id: text,
  target_id: text,
  request_hash: text,
  binding_id: text,
  owner: text,
  manager: text,
  generation: positiveInteger,
  binding_revision: positiveInteger,
  binding_digest: text,
  sandbox_digest: text,
  workspace_digest: text,
  admitted_at: positiveInteger,
  expires_at: positiveInteger,
});
export type BindingPreparationIntent = Output<typeof BindingPreparationIntentSchema>;

export const InspectBindingPreparationSchema = object({
  schema_version: positiveInteger,
  admission_id: text,
  target_id: text,
  request_hash: text,
  owner: text,
  admitted_at: positiveInteger,
  expires_at: positiveInteger,
  agent_key: text,
});
export type InspectBindingPreparation = Output<typeof InspectBindingPreparationSchema>;

export const PrepareBindingSchema = object({
  schema_version: positiveInteger,
  intent: BindingPreparationIntentSchema,
  agent_key: text,
});
export type PrepareBinding = Output<typeof PrepareBindingSchema>;

export const BindingPreparationInspectionSchema = tagged(
  {
    pinned: object({
      state: enumeration('pinned'),
      version: positiveInteger,
      binding_id: text,
      generation: positiveInteger,
    }),
    pending: object({ state: enumeration('pending'), intent: BindingPreparationIntentSchema }),
  },
  'state',
);
export type BindingPreparationInspection = Output<typeof BindingPreparationInspectionSchema>;

export const BindingPreparationReceiptSchema = object({
  version: positiveInteger,
  operation_id: text,
  intent_fingerprint: text,
  generation: positiveInteger,
  sandbox_id: text,
  workspace_id: text,
});
export type BindingPreparationReceipt = Output<typeof BindingPreparationReceiptSchema>;
