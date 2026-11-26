import { agentStatus, failure, sandboxStatus } from './types.js';
import { runtimeRecoveryProof, runtimeRecoveryReceipt } from './runtime-recovery.js';
import {
  array,
  enumeration,
  integer,
  object,
  record,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';

const simpleSteps = [
  'provision_runtime',
  'run_install',
  'mint_initial_credential',
  'deliver_initial_credential',
  'activate',
  'wait_for_credential_rotation',
  'mint_rotated_credential',
  'deliver_rotated_credential',
  'revoke_rotated_credential',
  'finish_credential_rotation',
  'run_lifecycle_operation',
  'stop_agents',
  'revoke_retirement_credentials',
  'teardown_runtime',
  'mark_retired',
  'retry_retired_revocations',
  'wait_for_purge',
  'purge',
  'none',
] as const;
const simpleStep = object({ kind: enumeration(...simpleSteps) });
const workspaceStep = object({ kind: enumeration('create_profile_workspace'), index: integer });
export const sandboxWorkStep = (value: unknown) =>
  record(value).kind === 'create_profile_workspace' ? workspaceStep(value) : simpleStep(value);
export type SandboxWorkStep = Output<typeof sandboxWorkStep>;

export const sandboxEventKind = tagged({
  created: variant('created', {}),
  work_step_completed: variant('work_step_completed', { step: sandboxWorkStep }),
  work_step_failed: variant('work_step_failed', { step: sandboxWorkStep, failure }),
  status_changed: variant('status_changed', { from: sandboxStatus, to: sandboxStatus }),
  workspace_created: variant('workspace_created', { workspace_id: text }),
  workspace_ready: variant('workspace_ready', { workspace_id: text, resolved_commit: text }),
  workspace_failed: variant('workspace_failed', { workspace_id: text, failure }),
  workspace_removed: variant('workspace_removed', { workspace_id: text }),
  worktree_created: variant('worktree_created', { workspace_id: text, worktree_id: text }),
  worktree_removed: variant('worktree_removed', { workspace_id: text, worktree_id: text }),
  agent_created: variant('agent_created', {
    workspace_id: text,
    worktree_id: text,
    agent_id: text,
  }),
  agent_status_changed: variant('agent_status_changed', {
    agent_id: text,
    from: agentStatus,
    to: agentStatus,
  }),
  agent_deleted: variant('agent_deleted', { agent_id: text }),
  credential_changed: variant('credential_changed', { key_generation: integer }),
  service_access_granted: variant('service_access_granted', {
    grant: object({
      services: array(text),
      authorized_by: text,
      authorized_at: text,
      credential_generation_floor: integer,
    }),
  }),
  service_access_revised: variant('service_access_revised', {
    scope: object({
      revision: integer,
      services: array(text),
      authorized_by: text,
      authorized_at: text,
      credential_generation_floor: integer,
    }),
  }),
  checkpoint_sealed: variant('checkpoint_sealed', { object_reference: text }),
  runtime_recovery_admitted: variant('runtime_recovery_admitted', {
    previous_status: sandboxStatus,
    target_runtime_version: text,
    proof: runtimeRecoveryProof,
  }),
  runtime_recovery_quiesced: variant('runtime_recovery_quiesced', {
    receipt: runtimeRecoveryReceipt,
  }),
  revived: variant('revived', {}),
  home_restored: variant('home_restored', {
    restore_id: text,
    backup_id: text,
    restored_to: text,
  }),
  rotated: variant('rotated', { runtime_version: text }),
  failed: variant('failed', { failure }),
});
export type SandboxEventKind = Output<typeof sandboxEventKind>;
export const sandboxEvent = object({
  sandbox_id: text,
  seq: integer,
  appended_at: text,
  kind: sandboxEventKind,
});
export type SandboxEvent = Output<typeof sandboxEvent>;
