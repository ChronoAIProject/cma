import { firstMessageDelivery } from '../agents/types.js';
import { environmentVariable, thinkingEffort } from '../profiles/types.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import {
  triggerAvailability,
  triggerButtonLabel,
  triggerTheme,
} from '../trigger-authoring/types.js';

export const launchState = enumeration(
  'draft',
  'resolving',
  'sandbox_pending',
  'workspace_pending',
  'agent_pending',
  'message_pending',
  'ready',
  'completed',
  'failed_retryable',
  'failed_terminal',
  'cancelled',
);
export type LaunchState = Output<typeof launchState>;
export const launchStepKey = enumeration(
  'signing_in',
  'checking_requirements',
  'sandbox',
  'workspace',
  'agent',
  'first_message',
  'ready',
);
export type LaunchStepKey = Output<typeof launchStepKey>;
export const launchStepStatus = enumeration(
  'pending',
  'running',
  'completed',
  'blocked',
  'retryable',
  'failed',
);
export type LaunchStepStatus = Output<typeof launchStepStatus>;
export const launchStep = object({
  key: launchStepKey,
  status: launchStepStatus,
  safe_detail: text,
  started_at: optional(text),
  completed_at: optional(text),
});
export type LaunchStep = Output<typeof launchStep>;
export const launchRepeatChoice = enumeration('resume', 'new');
export type LaunchRepeatChoice = Output<typeof launchRepeatChoice>;
export const serviceRequirementState = enumeration('satisfied', 'inactive', 'missing', 'unknown');
export type ServiceRequirementState = Output<typeof serviceRequirementState>;
export const serviceRequirement = object({ service: text, state: serviceRequirementState });
export type ServiceRequirement = Output<typeof serviceRequirement>;
export const launchRequirements = object({ services: array(serviceRequirement), checked_at: text });
export const serviceConnectionMetadata = object({
  service: text,
  state: serviceRequirementState,
  display_name: optional(text),
  connection_mode: optional(enumeration('hosted', 'manual', 'unknown')),
  setup_url: optional(text),
});
export type ServiceConnectionMetadata = Output<typeof serviceConnectionMetadata>;
export const launchConnectionRequirements = object({
  services: array(serviceConnectionMetadata),
  checked_at: text,
});
export type LaunchConnectionRequirements = Output<typeof launchConnectionRequirements>;
export const launchReviewNeed = tagged({
  connection: variant('connection', { service: text }),
  trust_confirmation: variant('trust_confirmation', { commands_digest: text }),
  owner_requires_review: variant('owner_requires_review', {}),
  repeat_choice: variant('repeat_choice', {}),
});
export type LaunchReviewNeed = Output<typeof launchReviewNeed>;
export const launchBlockedReason = tagged({
  connection_required: variant('connection_required', { service: text }),
  orchestration_paused: variant('orchestration_paused', {}),
  sandbox_paused_reviving: variant('sandbox_paused_reviving', {}),
  recovery_required: variant('recovery_required', {}),
});
export type LaunchBlockedReason = Output<typeof launchBlockedReason>;
export const launchFailure = object({
  code: text,
  safe_detail: text,
  retryable: boolean,
  occurred_at: text,
  remediation: optional(text),
});
export type LaunchFailure = Output<typeof launchFailure>;
export const resolvedLaunchResource = tagged(
  {
    pending: object({ state: enumeration('pending') }),
    reused: object({ state: enumeration('reused'), id: text }),
    created: object({ state: enumeration('created'), id: text }),
  },
  'state',
);
export const launchTrustDisclosure = object({
  resource: enumeration('sandbox', 'workspace'),
  name: text,
  commands_digest: text,
  commands: array(text),
  environment_variables: array(environmentVariable),
});
export type LaunchTrustDisclosure = Output<typeof launchTrustDisclosure>;
export const launch = object({
  id: text,
  trigger_id: text,
  revision_digest: text,
  public_id: text,
  placement_id: optional(text),
  referrer_origin: optional(text),
  state: launchState,
  steps: array(launchStep),
  resolution: object({
    sandbox: resolvedLaunchResource,
    workspace: resolvedLaunchResource,
    agent: resolvedLaunchResource,
    choice_required: boolean,
  }),
  requirements: launchRequirements,
  review: object({ needs: array(launchReviewNeed), can_start_now: boolean }),
  trust: object({
    version: integer,
    confirmation_digest: optional(text),
    sandbox_digest: optional(text),
    workspace_digest: optional(text),
    disclosures: array(launchTrustDisclosure),
  }),
  first_message: firstMessageDelivery,
  confirmations: array(
    object({
      surface: enumeration('landing', 'review'),
      trust_confirmed_digest: optional(text),
      acknowledgements: array(text),
      confirmed_at: text,
    }),
  ),
  blocked_reason: optional(launchBlockedReason),
  failure: optional(launchFailure),
  event_head: integer,
  created_at: text,
  updated_at: text,
  started_at: optional(text),
  ready_at: optional(text),
  revision: integer,
});
export type Launch = Output<typeof launch>;
const triggerTrustDisclosure = tagged(
  {
    sandbox: object({
      resource: enumeration('sandbox'),
      commands_digest: text,
      commands: array(text),
      environment_variables: array(environmentVariable),
    }),
    workspace: object({
      resource: enumeration('workspace'),
      name: text,
      commands_digest: text,
      commands: array(text),
    }),
  },
  'resource',
);
export const triggerPreview = object({
  title: text,
  description: text,
  button_label: triggerButtonLabel,
  theme: triggerTheme,
  always_review: optional(boolean),
  signup_hint: optional(text),
  publisher_display_name: text,
  what_happens: array(text),
  availability: triggerAvailability,
  trust: object({ disclosures: array(triggerTrustDisclosure) }),
  technical: object({
    provider: enumeration('codex'),
    model: optional(text),
    effort: thinkingEffort,
    size: object({ cpu_millis: integer, memory_mib: integer }),
    services: array(text),
  }),
  first_message: text,
});
export type TriggerPreview = Output<typeof triggerPreview>;
export const createLaunchRequest = object({
  placement_id: optional(text),
  referrer_origin: optional(text),
  start_if_ready: boolean,
  choice: optional(launchRepeatChoice),
});
export type CreateLaunchRequest = Output<typeof createLaunchRequest>;
export const startLaunchRequest = object({
  trust_confirmed_digest: optional(text),
  acknowledgements: optional(array(text)),
});
export type StartLaunchRequest = Output<typeof startLaunchRequest>;
export const existingLaunchAgent = object({
  agent_id: text,
  title: optional(nullable(text)),
  created_at: text,
  first_message_excerpt: text,
});
export type ExistingLaunchAgent = Output<typeof existingLaunchAgent>;
export const serviceConnectionState = enumeration(
  'pending',
  'satisfied',
  'expired',
  'cancelled',
  'failed',
);
export type ServiceConnectionState = Output<typeof serviceConnectionState>;
export const createdServiceConnection = object({
  handoff_id: text,
  hosted_url: text,
  expires_at: text,
});
export type CreatedServiceConnection = Output<typeof createdServiceConnection>;
export const serviceConnectionStatus = object({
  handoff_id: text,
  launch_id: text,
  service_id: text,
  state: serviceConnectionState,
  expires_at: text,
  updated_at: text,
});
export type ServiceConnectionStatus = Output<typeof serviceConnectionStatus>;
export const createServiceConnectionRequest = object({
  service_id: text,
  launch_id: text,
  intent_id: text,
});
export type CreateServiceConnectionRequest = Output<typeof createServiceConnectionRequest>;
