import { boolean, integer, object, text, type Output } from '../../schema.js';
import { EventWorkerProfileSchema } from './profile.js';
import { SecretSchema } from './primitives.js';

export const BindingProfileDriftSchema = object({
  assigned_profile_digest: text,
  prompt: boolean,
  skills: boolean,
  model: boolean,
});
export type BindingProfileDrift = Output<typeof BindingProfileDriftSchema>;

export const DesiredProfileDriftSchema = object({
  profile_digest: text,
  prompt: boolean,
  skills: boolean,
  model: boolean,
  service_scope: boolean,
});
export type DesiredProfileDrift = Output<typeof DesiredProfileDriftSchema>;

export const BaselineRequestSchema = object({ expected_version: integer });
export type BaselineRequest = Output<typeof BaselineRequestSchema>;

export const RevisionRequestSchema = object({
  expected_version: integer,
  profile: EventWorkerProfileSchema,
  agent_key: SecretSchema,
});
export type RevisionRequest = Output<typeof RevisionRequestSchema>;

export const RevisionReceiptSchema = object({
  channel_id: text,
  version: integer,
  profile_revision: integer,
  profile_digest: text,
  baseline_complete: boolean,
});
export type RevisionReceipt = Output<typeof RevisionReceiptSchema>;
