import { firstMessageDelivery } from '../agents/types.js';
import {
  boolean,
  enumeration,
  integer,
  object,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import {
  launchBlockedReason,
  launchFailure,
  launchRepeatChoice,
  launchState,
  launchStep,
  serviceRequirementState,
} from './types.js';

export const launchEventKind = tagged({
  created: variant('created', {}),
  confirmation_recorded: variant('confirmation_recorded', {}),
  state_changed: variant('state_changed', { from: launchState, to: launchState }),
  step_changed: variant('step_changed', { step: launchStep }),
  sandbox_resolved: variant('sandbox_resolved', { sandbox_id: text, reused: boolean }),
  sandbox_replaced: variant('sandbox_replaced', { previous_sandbox_id: text, sandbox_id: text }),
  workspace_resolved: variant('workspace_resolved', { workspace_id: text, reused: boolean }),
  workspace_replaced: variant('workspace_replaced', {
    previous_workspace_id: text,
    workspace_id: text,
  }),
  agent_resolved: variant('agent_resolved', { agent_id: text, reused: boolean }),
  repeat_choice_recorded: variant('repeat_choice_recorded', { choice: launchRepeatChoice }),
  requirement_changed: variant('requirement_changed', {
    service: text,
    state: serviceRequirementState,
  }),
  blocked: variant('blocked', { reason: launchBlockedReason }),
  resumed: variant('resumed', { reason: launchBlockedReason }),
  deferred: variant('deferred', { retry_at: text }),
  retry_scheduled: variant('retry_scheduled', { attempt: integer, retry_at: text }),
  effect_reconciled: variant('effect_reconciled', {
    kind: enumeration(
      'create_sandbox',
      'revive_sandbox',
      'repair_sandbox_service_grant',
      'create_workspace',
      'create_agent',
    ),
    result_id: text,
  }),
  first_message_changed: variant('first_message_changed', { state: firstMessageDelivery }),
  failed: variant('failed', { failure: launchFailure }),
  cancelled: variant('cancelled', {}),
  completed: variant('completed', {}),
});
export const launchEvent = object({
  launch_id: text,
  seq: integer,
  appended_at: text,
  kind: launchEventKind,
});
export type LaunchEvent = Output<typeof launchEvent>;
