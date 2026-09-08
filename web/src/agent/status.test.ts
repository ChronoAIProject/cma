import { expect, it } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { currentAgentStatus } from './status';

const empty = conversationFromTranscript({ agentId: 'agt_1', turns: [], throughSeq: 0 });
const later = '2026-09-06T00:10:00Z';
const latest = '2026-09-06T00:11:00Z';

it('does not let historical Ready override a newer stopped resource', () => {
  const conversation = foldAgentEvent(empty, {
    seq: 1,
    appendedAt: AGENT_DETAIL.createdAt,
    kind: { type: 'status_changed', to: 'ready' },
  });
  expect(currentAgentStatus({ ...AGENT_DETAIL, status: 'stopped' }, conversation)).toBe('stopped');
});

it('uses newer live status until a newer matching navigation observation arrives', () => {
  const conversation = foldLiveEvent(
    empty,
    {
      type: 'STATE_DELTA',
      threadId: 'agt_1',
      runId: 'run',
      timestamp: later,
      delta: [{ op: 'replace', path: '/status', value: 'working' }],
    },
    'agt_1',
  );
  expect(currentAgentStatus(AGENT_DETAIL, conversation)).toBe('working');
  const summary = {
    ...workspaceFixture().agents[0]!,
    status: 'stopped' as const,
    updatedAt: latest,
  };
  expect(currentAgentStatus(AGENT_DETAIL, conversation, summary)).toBe('stopped');
  for (const mismatch of [
    { id: 'other' },
    { sandboxId: 'other' },
    { workspaceId: 'other' },
    { worktreeId: 'other' },
    { updatedAt: 'invalid' },
  ])
    expect(currentAgentStatus(AGENT_DETAIL, conversation, { ...summary, ...mismatch })).toBe(
      'working',
    );
});

it('ignores unversioned and older status deltas while retaining other live content', () => {
  const current = { ...empty, agentStatus: 'stopped' as const, agentStatusAt: latest };
  for (const timestamp of [undefined, 'invalid', later]) {
    expect(
      foldLiveEvent(
        current,
        {
          type: 'STATE_DELTA',
          threadId: 'agt_1',
          runId: 'run',
          timestamp,
          delta: [{ op: 'replace', path: '/status', value: 'working' }],
        },
        'agt_1',
      ),
    ).toEqual(current);
  }
});

it('uses the resource snapshot when status timestamps are equal', () => {
  expect(
    currentAgentStatus(
      { ...AGENT_DETAIL, status: 'paused', updatedAt: later },
      { ...empty, agentStatus: 'ready', agentStatusAt: later },
    ),
  ).toBe('paused');
  expect(
    currentAgentStatus({ ...AGENT_DETAIL, status: 'stopped' }, empty, workspaceFixture().agents[0]),
  ).toBe('stopped');
});
