import { expect, it } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { navigationFixture, workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import { observedActivity } from './activityStatus';
import type { ReadyAgent } from './loadAgent';
import { observedTaskStatus } from '../shell/taskStatus';

const at = '2026-11-01T00:00:00Z';
function running(): ReadyAgent {
  return {
    detail: AGENT_DETAIL,
    workspace: workspaceFixture(),
    conversation: foldAgentEvent(
      conversationFromTranscript({ agentId: AGENT_DETAIL.id, turns: [], throughSeq: 0 }),
      {
        seq: 1,
        appendedAt: at,
        kind: { type: 'turn_started', turn_sequence: 1 },
      },
    ),
  };
}

it('withholds a completed label until history loads and preserves a failed read as unknown', () => {
  const state = running();
  expect(observedActivity({ ...state, historyLoading: true })).toBe('loading');
  expect(observedActivity({ ...state, historyUnavailable: true })).toBe('unknown');
  expect(observedActivity(state)).toBe('working');
});

it.each([
  ['turn_completed', 'completed'],
  ['turn_failed', 'failed'],
  ['turn_interrupted', 'interrupted'],
] as const)(
  'displays %s from recorded activity and leaves the resource status alone',
  (type, expected) => {
    const state = running();
    const conversation = foldAgentEvent(state.conversation, {
      seq: 2,
      appendedAt: at,
      kind: { type, turn_sequence: 1, failure: { class: 'runtime', detail: 'Synthetic failure' } },
    });
    expect(observedActivity({ ...state, conversation })).toBe(expected);
    expect(state.detail.status).toBe('ready');
    const newer = foldAgentEvent(conversation, {
      seq: 3,
      appendedAt: at,
      kind: { type: 'turn_started', turn_sequence: 3 },
    });
    const late = foldAgentEvent(newer, {
      seq: 4,
      appendedAt: at,
      kind: { type, turn_sequence: 1 },
    });
    expect(observedActivity({ ...state, conversation: late })).toBe('working');
  },
);

it('does not override equally recent or newer resource evidence, lifecycle barriers or another Agent', () => {
  const state = running();
  const summary = state.workspace.agents[0]!;
  expect(observedActivity(state, { ...summary, updatedAt: at })).toBeUndefined();
  expect(observedActivity(state, { ...summary, status: 'stopped', updatedAt: at })).toBeUndefined();
  expect(observedActivity(state, { ...summary, id: 'other-agent', updatedAt: at })).toBe('working');
  expect(
    observedActivity({
      ...state,
      detail: { ...state.detail, status: 'stopped' },
      historyLoading: true,
    }),
  ).toBeUndefined();
  const task = { agent: state.detail, status: 'loading' as const };
  const sandbox = navigationFixture()[0]!.sandbox;
  expect(observedTaskStatus(summary, sandbox, task)).toBe('loading');
  expect(observedTaskStatus(summary, { ...sandbox, status: 'paused' }, task)).toBe('paused');
  expect(observedTaskStatus({ ...summary, status: 'ready', taskStatus: 'stopped' }, sandbox)).toBe(
    'interrupted',
  );
  expect(observedTaskStatus({ ...summary, status: 'stopped' }, sandbox, task)).toBe('stopped');
  expect(
    observedTaskStatus({ ...summary, id: 'other-agent', taskStatus: 'completed' }, sandbox, task),
  ).toBe('completed');
});
