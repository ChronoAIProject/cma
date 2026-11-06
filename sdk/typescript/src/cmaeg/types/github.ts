import { ProtocolError } from '../../errors.js';
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
import { SecretSchema } from './primitives.js';

const timestamp = (value: unknown): number => {
  if (typeof value !== 'number' || !Number.isSafeInteger(value))
    throw new ProtocolError('expected a safe integer timestamp');
  return value;
};

export const GitHubSourceSchema = object({
  type: enumeration('github'),
  app_ref: text,
  installation_ids: array(integer),
  repository_ids: array(integer),
  deny_repository_ids: optional(array(integer)),
  events: array(text),
  actions: array(text),
  prompt_template: optional(nullable(text)),
});

export const CredentialStatusSchema = object({
  present: boolean,
  version: optional(nullable(integer)),
});
export type CredentialStatus = Output<typeof CredentialStatusSchema>;

export const WebhookCredentialOverlapSchema = object({
  version: integer,
  valid_until: timestamp,
});
export type WebhookCredentialOverlap = Output<typeof WebhookCredentialOverlapSchema>;

export const WebhookCredentialStatusSchema = object({
  present: boolean,
  version: optional(nullable(integer)),
  previous: optional(nullable(WebhookCredentialOverlapSchema)),
});
export type WebhookCredentialStatus = Output<typeof WebhookCredentialStatusSchema>;

export const AppVerificationSchema = object({
  verified_by: text,
  verified_at: timestamp,
  evidence_ref: text,
});
export type AppVerification = Output<typeof AppVerificationSchema>;

export const AppIntegrationSchema = object({
  app_ref: text,
  host: text,
  app_id: integer,
  display_name: text,
  version: integer,
  status: text,
  webhook_url: text,
  webhook_credential: WebhookCredentialStatusSchema,
  verification_credential: CredentialStatusSchema,
  verification: optional(nullable(AppVerificationSchema)),
  retired_at: optional(nullable(timestamp)),
});
export type AppIntegration = Output<typeof AppIntegrationSchema>;

export const AppListSchema = object({
  items: array(AppIntegrationSchema),
  next_cursor: optional(nullable(text)),
});
export type AppList = Output<typeof AppListSchema>;

export const CreateAppRequestSchema = object({
  app_id: integer,
  display_name: text,
  webhook_secret: optional(nullable(SecretSchema)),
});
export type CreateAppRequest = Output<typeof CreateAppRequestSchema>;

export const UpdateAppRequestSchema = object({ expected_version: integer, display_name: text });
export type UpdateAppRequest = Output<typeof UpdateAppRequestSchema>;

export const VerifyAppRequestSchema = object({ expected_version: integer, evidence_ref: text });
export type VerifyAppRequest = Output<typeof VerifyAppRequestSchema>;

export const WebhookSecretRequestSchema = object({
  expected_version: integer,
  webhook_secret: SecretSchema,
  overlap_seconds: optional(integer),
});
export type WebhookSecretRequest = Output<typeof WebhookSecretRequestSchema>;

export const GitHubCapabilitiesSchema = object({
  store_ready: boolean,
  dynamic_app_registration: boolean,
  native_app_webhook: boolean,
  webhook_secret_rotation: boolean,
  app_identity_verification: boolean,
  ownership_verification: boolean,
  automatic_reconciliation: boolean,
  durable_dispatch: optional(boolean),
});
export type GitHubCapabilities = Output<typeof GitHubCapabilitiesSchema>;

export const BindRequestSchema = object({
  app_ref: text,
  installation_id: integer,
  proof_ref: optional(text),
  account_id: optional(integer),
  repository_ids: optional(array(integer)),
});
export type BindRequest = Output<typeof BindRequestSchema>;

export const BindingApprovalSchema = object({
  verified_by: text,
  evidence_ref: text,
  approved_at: timestamp,
  authority_revision: integer,
  repository_ids: array(integer),
});
export type BindingApproval = Output<typeof BindingApprovalSchema>;

export const VerifyBindingRequestSchema = object({
  expected_version: integer,
  evidence_ref: text,
  repository_ids: array(integer),
});
export type VerifyBindingRequest = Output<typeof VerifyBindingRequestSchema>;

export const RetireAppRequestSchema = object({ expected_version: integer });
export type RetireAppRequest = Output<typeof RetireAppRequestSchema>;

export const BindingSchema = object({
  binding_id: text,
  app_ref: text,
  app_id: integer,
  installation_id: integer,
  account_id: integer,
  status: text,
  authority_revision: integer,
  verified_until: timestamp,
  verified_by: text,
  verification_ref: text,
  repository_ids: array(integer),
  version: optional(integer),
  requested_repository_ids: optional(array(integer)),
  approved_repository_ids: optional(array(integer)),
  excluded_repository_ids: optional(array(integer)),
  approval: optional(nullable(BindingApprovalSchema)),
});
export type Binding = Output<typeof BindingSchema>;

export const ReceiptSchema = object({
  receipt_id: text,
  app_ref: text,
  installation_id: optional(nullable(integer)),
  delivery_id: text,
  repository_id: optional(nullable(integer)),
  event: text,
  action: optional(nullable(text)),
  status: text,
  reason: optional(nullable(text)),
  received_at: timestamp,
  raw_body_expires_at: timestamp,
  retained_until: optional(nullable(timestamp)),
  body_sha256: text,
  task_ids: array(text),
  authority_revision: integer,
});
export type Receipt = Output<typeof ReceiptSchema>;

export const TaskResultStatusSchema = enumeration('succeeded', 'no_change');
export type TaskResultStatus = Output<typeof TaskResultStatusSchema>;

export const TaskResultSchema = object({
  status: TaskResultStatusSchema,
  pull_request: optional(nullable(text)),
  branch: optional(nullable(text)),
  commit: optional(nullable(text)),
});
export type TaskResult = Output<typeof TaskResultSchema>;

export const TaskSchema = object({
  task_id: text,
  status: text,
  response_id: optional(nullable(text)),
  channel_id: text,
  record_id: text,
  event_id: text,
  target_count: integer,
  accepted_at: timestamp,
  retained_until: timestamp,
  result: optional(nullable(TaskResultSchema)),
});
export type Task = Output<typeof TaskSchema>;
