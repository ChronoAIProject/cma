import { expect, it } from 'vitest';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from '../agent/conversation';
import { externalApprovalFixture } from '../agent/externalApproval.fixture';
import type { ConversationState, ReasoningBlock, ToolCall } from '../agent/model';
import { agentVisual } from './agentVisual';

const agentId = 'test-agent';
const at = '2026-01-01T00:01:00.000Z';
const observedAt = Date.parse(at);
const active = foldAgentEvent(conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }), {
  seq: 7,
  appendedAt: at,
  kind: { type: 'turn_started', turn_sequence: 7 },
});
const tool: ToolCall = { id: 'tool', runId: '7', title: 'Read file', status: 'running' };
const reasoning: ReasoningBlock = {
  id: 'reasoning',
  runId: '7',
  content: 'Checking the file',
  status: 'streaming',
};
const external = { ...externalApprovalFixture, runId: '7' };
const native = { id: 'native', runId: '7', reason: 'confirmation' };
const approvalLabel = 'Waiting for approval';
const view = (
  changes: Partial<ConversationState>,
  task: 'working' | 'waiting_approval' = 'working',
) =>
  agentVisual({
    status: 'ready',
    task,
    turnId: changes.latestTurnId ?? '7',
    activity: { conversation: { ...active, ...changes }, observedAt },
  });

it.each([
  ['external approval', { approvals: [external], toolCalls: [tool] }, 'question', approvalLabel],
  ['native approval', { approvals: [native], toolCalls: [tool] }, 'question', approvalLabel],
  ['pending tool', { toolCalls: [tool] }, 'working', 'Running a tool'],
  ['streaming reasoning', {}, 'idea', 'Thinking'],
  ['finished tool', { toolCalls: [{ ...tool, status: 'completed' }] }, 'idea', 'Thinking'],
] as const)('selects %s by current-turn precedence', (_stage, changes, action, label) => {
  expect(view({ reasoning: [reasoning], ...changes })).toEqual({ action, label, canAnimate: true });
});

it.each([
  ['old external approval', { approvals: [{ ...external, runId: '6' }] }],
  ['old native approval', { approvals: [{ ...native, runId: '6' }] }],
  ['unbound native approval', { approvals: [{ ...native, runId: undefined }] }],
  ['resolved native approval', { approvals: [{ ...native, decision: 'approved' }] }],
  ['resolved external approval', { approvals: [{ ...external, decision: 'approved' }] }],
  ['expired external approval', { approvals: [{ ...external, deadlineAt: at }] }],
  ['old pending tool', { toolCalls: [{ ...tool, runId: '6' }] }],
  ['old streaming reasoning', { reasoning: [{ ...reasoning, runId: '6' }] }],
  ['empty streaming reasoning', { reasoning: [{ ...reasoning, content: ' ' }] }],
] as const)('ignores %s', (_stage, changes) => {
  expect(view(changes, 'waiting_approval')).toMatchObject({
    action: 'working',
    label: 'Waiting for a response',
  });
});

it.each([
  [{ reasoning: [{ ...reasoning, status: 'complete' }] }, 'Waiting for more activity'],
  [
    { plans: [{ id: 'plan', runId: '7', entries: [{ title: 'Inspect', status: 'in_progress' }] }] },
    'Waiting for more activity',
  ],
  [
    {
      messages: [
        { id: 'reply', runId: '7', role: 'assistant', content: 'What next?', status: 'streaming' },
      ],
    },
    'Responding',
  ],
] as const)('keeps ordinary work for %j', (changes, label) => {
  expect(view(changes)).toMatchObject({ action: 'working', label });
});

it.each([
  foldAgentEvent(active, {
    seq: 8,
    appendedAt: at,
    kind: { type: 'approval_requested', approval_id: 'native' },
  }),
  foldLiveEvent(
    active,
    {
      threadId: agentId,
      type: 'RUN_FINISHED',
      runId: '7',
      outcome: { type: 'interrupt', interrupts: [{ id: 'native' }] },
    },
    agentId,
  ),
  foldLiveEvent(
    active,
    {
      threadId: agentId,
      type: 'STATE_DELTA',
      runId: '7',
      delta: [{ op: 'add', path: '/pendingApprovals/native', value: { id: 'native' } }],
    },
    agentId,
  ),
])('projects native approval run identity for %j', (conversation) => {
  expect(conversation.approvals[0]?.runId).toBe('7');
  expect(view(conversation).action).toBe('question');
  const next = foldAgentEvent(conversation, {
    seq: 9,
    appendedAt: at,
    kind: { type: 'turn_started', turn_sequence: 9 },
  });
  expect(view(next, 'waiting_approval').action).toBe('working');
});

it('keeps current work when an older turn completion and acceptance arrive', () => {
  const conversation = foldAgentEvent(active, {
    seq: 8,
    appendedAt: at,
    kind: { type: 'turn_completed', turn_sequence: 6 },
  });
  const acceptance = {
    task_id: 'test-task',
    turn_sequence: 6,
    verdict: { status: 'succeeded' as const },
    unconfirmed_outputs: [],
  };
  const activity = { conversation, observedAt };
  expect(conversation.activeRunId).toBe('7');
  expect(
    agentVisual({ status: 'ready', task: 'working', turnId: '7', acceptance, activity }).action,
  ).toBe('working');
  expect(
    agentVisual({ status: 'ready', task: 'task_accepted', turnId: '6', acceptance, activity })
      .action,
  ).toBe('ready');
  const ended = { ...conversation, activeRunId: undefined };
  expect(
    agentVisual({
      status: 'ready',
      task: 'task_accepted',
      turnId: '6',
      acceptance,
      activity: { conversation: ended, observedAt },
    }).label,
  ).toBe('Turn ended');
});

it('keeps unknown, ended, or parent-held states outside the live visual', () => {
  const conversation = {
    ...active,
    approvals: [external],
    toolCalls: [tool],
    reasoning: [reasoning],
  };
  const activity = { conversation, observedAt };
  expect(agentVisual({ status: 'ready', task: 'unknown', activity }).label).toBe(
    'Status unavailable',
  );
  expect(
    agentVisual({ status: 'ready', task: 'working', sandbox: { status: 'paused' }, activity })
      .label,
  ).toBe('Paused');
  const ended = { ...conversation, turnOutcomes: { 7: 'completed' as const } };
  expect(
    agentVisual({
      status: 'ready',
      task: 'completed',
      activity: { conversation: ended, observedAt },
    }).label,
  ).toBe('Turn ended');
});
