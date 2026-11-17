import type {
  LaunchBlockedReason,
  LaunchState,
  LaunchStepStatus,
  ServiceRequirementState,
} from '@cma/client';

export const stepStates = {
  pending: 'Waiting',
  running: 'In progress',
  completed: 'Done',
  blocked: 'Needs you',
  retryable: 'Waiting',
  failed: "Didn't work",
} satisfies Record<LaunchStepStatus, string>;
export const requestStates = {
  draft: 'Needs you',
  resolving: 'In progress',
  sandbox_pending: 'In progress',
  workspace_pending: 'In progress',
  agent_pending: 'In progress',
  message_pending: 'In progress',
  ready: 'Ready',
  completed: 'Done',
  failed_retryable: "Didn't work",
  failed_terminal: "Didn't work",
  cancelled: 'Cancelled',
} satisfies Record<LaunchState, string>;
export const blockedStates = {
  connection_required: 'Needs you',
  orchestration_paused: 'Paused by CMA',
  sandbox_paused_reviving: 'In progress',
  recovery_required: 'Needs you',
} satisfies Record<LaunchBlockedReason['type'], string>;
export const requirementStates = {
  satisfied: 'Connected',
  inactive: 'Needs you',
  missing: 'Needs you',
  unknown: 'Checking',
} satisfies Record<ServiceRequirementState, string>;
