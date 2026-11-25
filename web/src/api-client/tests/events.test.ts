// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { proof, receipt } from '../../test/runtimeRecovery';
import { ProtocolError, type AgentEventKind, type SandboxEventKind } from '../index.js';
import { agentEventKind } from '../agents/events.js';
import { sandboxEventKind, sandboxWorkStep } from '../instances/events.js';
import { dictionary, integer, object, optional, text } from '../schema.js';

const failure = { class: 'runtime', detail: 'Unavailable' };
const agentEvents = {
  access_redacted: { type: 'access_redacted' },
  created: { type: 'created', workspace_id: 'workspace-1', worktree_id: 'worktree-1' },
  conversation_opened: {
    type: 'conversation_opened',
    conversation_id: 'conversation-1',
    runtime_generation: 1,
  },
  runtime_generation_retired: {
    type: 'runtime_generation_retired',
    previous_generation: 1,
    runtime_generation: 2,
  },
  work_step_completed: { type: 'work_step_completed', step: 'publish_ready' },
  work_step_failed: { type: 'work_step_failed', step: 'open_conversation', failure },
  skill_installation_changed: {
    type: 'skill_installation_changed',
    skill_id: 'skill-1',
    status: 'installed',
  },
  turn_started: { type: 'turn_started', turn_sequence: 1 },
  turn_metadata_set: {
    type: 'turn_metadata_set',
    turn_sequence: 1,
    metadata: { source: 'review' },
  },
  owner_message: { type: 'owner_message', content: 'Review' },
  agent_message: { type: 'agent_message', content: 'Reviewed' },
  turn_completed: { type: 'turn_completed', turn_sequence: 1 },
  turn_failed: { type: 'turn_failed', turn_sequence: 1, failure },
  turn_interrupted: { type: 'turn_interrupted', turn_sequence: 1 },
  tool_call_started: {
    type: 'tool_call_started',
    call_id: 'call-1',
    title: 'Read',
    kind: 'read',
    status: 'running',
  },
  tool_call_updated: {
    type: 'tool_call_updated',
    call_id: 'call-1',
    status: 'completed',
    title: 'Read file',
  },
  approval_requested: {
    type: 'approval_requested',
    approval_id: 'approval-1',
    action: 'Write',
    detail: 'Update file',
  },
  tool_output_reported: {
    type: 'tool_output_reported',
    call_id: 'call-1',
    output: { text: 'native-computer-proof\n', truncated: false },
  },
  approval_resolved: { type: 'approval_resolved', approval_id: 'approval-1', decision: 'allow' },
  operation_delivery_changed: {
    type: 'operation_delivery_changed',
    operation_sequence: 1,
    operation: 'send_message',
    state: 'execution_observed',
  },
  usage: {
    type: 'usage',
    input_tokens: 10,
    output_tokens: 20,
    context_used: 30,
    context_window: 100,
  },
  runtime_metadata_changed: {
    type: 'runtime_metadata_changed',
    metadata: { working_directory: '/home/cma', description: 'Review' },
  },
  usage_updated: {
    type: 'usage_updated',
    input_tokens: 10,
    output_tokens: 2,
    context_used: 12,
    context_window: null,
    turn_sequence: null,
    turn_input_tokens: 0,
    turn_output_tokens: 0,
  },
  mode_changed: { type: 'mode_changed', mode: 'review' },
  plan_set: { type: 'plan_set', entries: [{ title: 'Review', status: 'in_progress' }] },
  config_options_set: {
    type: 'config_options_set',
    options: [{ id: 'mode', name: 'Mode', kind: 'select' }],
  },
  available_commands_set: {
    type: 'available_commands_set',
    commands: [{ name: 'review', description: 'Review changes' }],
  },
  conversation_gap: { type: 'conversation_gap', frames: 2 },
  sign_in_required: {
    type: 'sign_in_required',
    url: 'https://signin.example.test',
    code: 'review-code',
    message: 'Sign in',
  },
  status_changed: { type: 'status_changed', from: 'ready', to: 'working' },
  first_message_changed: {
    type: 'first_message_changed',
    state: 'execution_observed',
    recovery_generation: 1,
  },
  title_set: { type: 'title_set', title: 'Review', source: 'owner' },
  stopped: { type: 'stopped', reason: 'owner' },
  deleted: { type: 'deleted' },
} satisfies Record<AgentEventKind['type'], AgentEventKind>;

const sandboxEvents = {
  created: { type: 'created' },
  work_step_completed: {
    type: 'work_step_completed',
    step: { kind: 'create_profile_workspace', index: 0 },
  },
  work_step_failed: { type: 'work_step_failed', step: { kind: 'run_install' }, failure },
  status_changed: { type: 'status_changed', from: 'provisioning', to: 'active' },
  workspace_created: { type: 'workspace_created', workspace_id: 'workspace-1' },
  workspace_ready: {
    type: 'workspace_ready',
    workspace_id: 'workspace-1',
    resolved_commit: 'commit-1',
  },
  workspace_failed: { type: 'workspace_failed', workspace_id: 'workspace-1', failure },
  workspace_removed: { type: 'workspace_removed', workspace_id: 'workspace-1' },
  worktree_created: {
    type: 'worktree_created',
    workspace_id: 'workspace-1',
    worktree_id: 'worktree-1',
  },
  worktree_removed: {
    type: 'worktree_removed',
    workspace_id: 'workspace-1',
    worktree_id: 'worktree-1',
  },
  agent_created: {
    type: 'agent_created',
    workspace_id: 'workspace-1',
    worktree_id: 'worktree-1',
    agent_id: 'agent-1',
  },
  agent_status_changed: {
    type: 'agent_status_changed',
    agent_id: 'agent-1',
    from: 'ready',
    to: 'working',
  },
  agent_deleted: { type: 'agent_deleted', agent_id: 'agent-1' },
  credential_changed: { type: 'credential_changed', key_generation: 2 },
  service_access_granted: {
    type: 'service_access_granted',
    grant: {
      services: ['api-github'],
      authorized_by: 'owner-1',
      authorized_at: '2026-09-09T00:00:00Z',
      credential_generation_floor: 2,
    },
  },
  service_access_revised: {
    type: 'service_access_revised',
    scope: {
      revision: 2,
      services: ['chrono-llm-public', 'ornn-api'],
      authorized_by: 'owner-1',
      authorized_at: '2026-10-15T00:00:00Z',
      credential_generation_floor: 4,
    },
  },
  checkpoint_sealed: { type: 'checkpoint_sealed', object_reference: 'checkpoint-1' },
  runtime_recovery_admitted: {
    type: 'runtime_recovery_admitted',
    previous_status: 'failed',
    target_runtime_version: 'runtime-next',
    proof,
  },
  runtime_recovery_quiesced: { type: 'runtime_recovery_quiesced', receipt },
  revived: { type: 'revived' },
  home_restored: {
    type: 'home_restored',
    restore_id: 'restore-1',
    backup_id: 'backup-1',
    restored_to: '2026-09-09T00:00:00Z',
  },
  rotated: { type: 'rotated', runtime_version: 'runtime-2' },
  failed: { type: 'failed', failure },
} satisfies Record<SandboxEventKind['type'], SandboxEventKind>;

describe('closed public event projections', () => {
  it.each(Object.entries(agentEvents))('parses the complete %s Agent event', (type, event) => {
    expect(event.type).toBe(type);
    expect(agentEventKind(event)).toEqual(event);
  });

  it.each(Object.entries(sandboxEvents))('parses the complete %s Sandbox event', (type, event) => {
    expect(event.type).toBe(type);
    expect(sandboxEventKind(event)).toEqual(event);
  });

  it('does not project private origin keys or runtime approval locators', () => {
    expect(
      agentEventKind({ ...agentEvents.turn_started, origin_key: 'private-operation' }),
    ).toEqual(agentEvents.turn_started);
    expect(
      agentEventKind({ ...agentEvents.approval_requested, runtime_locator: 'private-locator' }),
    ).toEqual(agentEvents.approval_requested);
  });

  it('rejects unknown prior status and incomplete home restoration', () => {
    expect(() =>
      sandboxEventKind({ ...sandboxEvents.runtime_recovery_admitted, previous_status: 'unknown' }),
    ).toThrow(ProtocolError);
    expect(() => sandboxEventKind({ type: 'home_restored' })).toThrow(ProtocolError);
  });

  it('rejects malformed, unknown and incomplete events with a field path', () => {
    for (const value of [
      null,
      [],
      {},
      { type: 'future_event' },
      { type: 'toString' },
      { type: 1 },
    ]) {
      expect(() => agentEventKind(value)).toThrow(ProtocolError);
    }
    expect(() => agentEventKind({ type: 'turn_started', turn_sequence: -1 })).toThrow(
      /turn_sequence/,
    );
    expect(() => agentEventKind({ type: 'plan_set', entries: [{ title: 'Review' }] })).toThrow(
      /entries: status/,
    );
    expect(() => sandboxWorkStep({ kind: 'create_profile_workspace' })).toThrow(/index/);
    expect(() => sandboxWorkStep({ kind: 'future_step' })).toThrow(ProtocolError);
  });

  it('parses optional fields without prototype pollution or swallowing parser bugs', () => {
    const parse = object({ count: integer, name: optional(text) });
    expect(parse({ count: 0 })).toEqual({ count: 0 });
    expect(() => parse({ count: 1, name: null })).toThrow(/name/);
    const values = dictionary(text)(JSON.parse('{"__proto__":"value"}') as unknown);
    expect(Object.hasOwn(values, '__proto__')).toBe(true);
    expect(Object.getPrototypeOf(values)).toBe(Object.prototype);
    const failure = new Error('parser implementation failed');
    expect(() =>
      object({
        value: () => {
          throw failure;
        },
      })({ value: 1 }),
    ).toThrow(failure);
  });
});
