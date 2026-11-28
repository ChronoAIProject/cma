import { expect, it } from 'vitest';
import type { TaskAcceptance } from '@cma/client';
import type { AgentStatus, SandboxStatus } from '../api/types';
import { observedActivity } from '../agent/activityStatus';
import { currentAgentStatus } from '../agent/status';
import { observedTaskStatus, type TaskStatus } from '../shell/taskStatus';
import { AGENT_DETAIL } from '../test/agent';
import { parkedRuntimeUpdateFixture, workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript } from '../agent/conversation';
import { agentVisual, type AgentVisualAction } from './agentVisual';

it.each([
  ['starting', 'preparing', 'Preparing', true],
  ['ready', 'ready', 'Ready', false],
  ['working', 'working', 'Working', true],
  ['waiting_approval', 'question', 'Waiting for approval', true],
  ['paused', 'rest', 'Paused', false],
  ['stopped', 'rest', 'Agent stopped', false],
  ['deleted', 'rest', 'Agent deleted', false],
  ['failed', 'failed', 'Agent failed', true],
] satisfies [AgentStatus, AgentVisualAction, string, boolean][])(
  'presents the AM lifecycle %s',
  (status, action, label, canAnimate) => {
    expect(agentVisual({ status })).toEqual({ action, label, canAnimate });
  },
);

it.each([
  ['loading', 'preparing', 'Loading activity', true],
  ['unknown', 'rest', 'Status unavailable', false],
  ['not_started', 'ready', 'Ready to start', false],
  ['completed', 'ready', 'Turn ended', false],
  ['failed', 'failed', 'This turn failed', true],
  ['interrupted', 'rest', 'Response interrupted', false],
] satisfies [TaskStatus, AgentVisualAction, string, boolean][])(
  'presents %s independently of the Ready resource lifecycle',
  (task, action, label, canAnimate) => {
    expect(agentVisual({ status: 'ready', task })).toEqual({ action, label, canAnimate });
  },
);

function accepted(verdict: TaskAcceptance['verdict']): TaskAcceptance {
  return { task_id: 'test-task', turn_sequence: 7, verdict, unconfirmed_outputs: [] };
}

it.each([
  [{ status: 'succeeded' }, 'love', 'Task complete', true, undefined],
  [
    { status: 'needs_attention', reason: 'required_deliverables_missing' },
    'question',
    'Needs attention',
    false,
    'Required outputs still need verification.',
  ],
  [
    { status: 'indeterminate', reason: 'history_incomplete' },
    'question',
    'Needs attention',
    false,
    'Part of the turn history is missing.',
  ],
  [
    { status: 'needs_attention', reason: 'execution_failed' },
    'failed',
    'Needs attention',
    false,
    'The turn failed before acceptance.',
  ],
] satisfies [TaskAcceptance['verdict'], AgentVisualAction, string, boolean, string | undefined][])(
  'presents the current turn acceptance %j',
  (verdict, action, label, canAnimate, detail) => {
    const acceptance = accepted(verdict);
    const row = { ...workspaceFixture().agents[0]!, taskStatus: 'completed' as const };
    const current = { agent: { ...AGENT_DETAIL, taskAcceptance: acceptance }, turnId: '7' };
    const task = observedTaskStatus(row, undefined, current);
    expect(agentVisual({ status: row.status, task, acceptance, turnId: current.turnId })).toEqual(
      detail === undefined ? { action, label, canAnimate } : { action, label, canAnimate, detail },
    );
  },
);

it.each(['8', undefined])('requires a matching turn for Task complete (%s)', (turnId) => {
  const acceptance = accepted({ status: 'succeeded' });
  expect(agentVisual({ status: 'ready', task: 'task_accepted', acceptance, turnId })).toEqual({
    action: 'ready',
    label: 'Turn ended',
    canAnimate: false,
  });
});

it('uses the current observations when a newer turn follows an earlier accepted result', () => {
  const acceptance = accepted({ status: 'succeeded' });
  const detail = { ...AGENT_DETAIL, taskAcceptance: acceptance };
  const conversation = {
    ...conversationFromTranscript({ agentId: detail.id, turns: [], throughSeq: 0 }),
    activeRunId: '8',
    turnTiming: { 8: { startedAt: '2026-11-01T00:00:00Z' } },
  };
  const workspace = workspaceFixture();
  const row = { ...workspace.agents[0]!, taskStatus: 'completed' as const };
  const status = currentAgentStatus(detail, conversation, row);
  const current = {
    agent: detail,
    status: observedActivity({ detail, conversation, workspace }),
    turnId: '8',
  };
  const task = observedTaskStatus(row, undefined, current);
  expect(agentVisual({ status, task, acceptance, turnId: current.turnId })).toEqual({
    action: 'working',
    label: 'Working',
    canAnimate: true,
  });
});

it.each([
  ['provisioning', 'preparing', 'Provisioning', true],
  ['installing', 'preparing', 'Installing', true],
  ['pausing', 'rest', 'Pausing', false],
  ['paused', 'rest', 'Paused', false],
  ['reviving', 'preparing', 'Resuming', true],
  ['rotating', 'preparing', 'Updating runtime', true],
  ['retiring', 'rest', 'Deleting', false],
  ['retired', 'rest', 'Deleted', false],
  ['failed', 'failed', 'Needs attention', false],
] satisfies [SandboxStatus, AgentVisualAction, string, boolean][])(
  'uses parent %s ahead of current work and a completed task',
  (status, action, label, canAnimate) => {
    const sandbox = { status };
    const expected = { action, label, canAnimate };
    expect(agentVisual({ sandbox, status: 'working', task: 'working' })).toEqual(expected);
    expect(
      agentVisual({
        sandbox,
        status: 'ready',
        task: 'task_accepted',
        acceptance: accepted({ status: 'succeeded' }),
        turnId: '7',
      }),
    ).toEqual(expected);
  },
);

it('uses the observed parent operation and preserves a terminal child', () => {
  const sandbox = { status: 'active', lifecycle: { operation: { purpose: 'pause' } } } as const;
  expect(agentVisual({ status: 'working', sandbox })).toEqual({
    action: 'rest',
    label: 'Pausing',
    canAnimate: false,
  });
  expect(agentVisual({ status: 'stopped', sandbox, task: 'working' }).label).toBe('Agent stopped');
  expect(
    agentVisual({ status: 'working', sandbox: { status: 'credential_rotating' } }).action,
  ).toBe('working');
});

it('shows a parked parent update as static attention', () => {
  expect(agentVisual({ status: 'working', sandbox: parkedRuntimeUpdateFixture() })).toEqual({
    action: 'failed',
    label: 'Runtime update needs attention',
    canAnimate: false,
  });
});

it('preserves the distinction between a stopped turn and a stopped Agent', () => {
  const row = { ...workspaceFixture().agents[0]!, taskStatus: 'stopped' as const };
  const task = observedTaskStatus(row, undefined);
  expect(agentVisual({ status: row.status, task }).label).toBe('Response interrupted');
  expect(agentVisual({ status: 'stopped', task }).label).toBe('Agent stopped');
});

it.each([undefined, 'unexpected' as AgentStatus])(
  'falls back to a neutral visual for %s',
  (status) => {
    expect(agentVisual({ status, task: 'task_accepted' })).toEqual(
      agentVisual({ status: 'ready', task: 'unknown' }),
    );
  },
);
