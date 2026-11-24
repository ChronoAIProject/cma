import { expect, it } from 'vitest';
import fixture from '../../test/externalApprovalProjection.fixture.json';
import {
  v2AgentEventPage,
  v2AgentEvent,
  v2AgentEventHead,
} from '../../api-client/agent-events-v2/types';
import { externalApprovalProjection } from '../../api-client/agent-events-v2/external-approval';
import { conversationFromTranscript } from '../conversation';
import type { ConversationState } from '../model';
import { ConversationHydration } from './hydration';
import { conversationFromHead } from './liveHead';
import { foldV2Item } from './projection';
import { withApprovalProjection } from './externalApprovalProjection';

const raw = fixture.waiting_snapshot;
const agent = raw.agent_id;
const policy = externalApprovalProjection(new Headers(fixture.response_headers), true);
const page = v2AgentEventPage(raw, agent, 100, undefined, true);
const frame = (data: unknown) => ({
  type: 'cma.agent.event' as const,
  data: v2AgentEvent(data, true),
});
const head = () =>
  conversationFromHead({
    ...v2AgentEventHead(
      {
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: agent,
        incarnation: raw.incarnation,
        through_cursor: raw.through_cursor,
        active_turn: raw.items[2],
      },
      agent,
    ),
    externalApprovalProjection: policy,
  });
const snapshot = () =>
  page.items.reduce<ConversationState>(
    foldV2Item,
    withApprovalProjection(
      {
        ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
        agentIncarnation: raw.incarnation,
      },
      policy,
    ),
  );

it('buffers a live resolution ahead of its historical request, including an intermediate head', () => {
  const hydration = new ConversationHydration();
  let state = hydration.merge(head(), agent, false);
  state = hydration.applyV2(state, frame(fixture.live_resolved.data), agent, true);
  expect(state.approvals).toEqual([]);
  expect(state.cursor).toBe(10);
  expect(hydration.merge(head(), agent, false).approvals).toEqual([]);
  const merged = hydration.merge(snapshot(), agent, true);
  expect(merged.approvals).toMatchObject([{ decision: 'approved', revision: 2 }]);
  expect(merged.activeRunId).toBe('3');
});

it('never exposes a late pending receipt before learning that history already resolved it', () => {
  const hydration = new ConversationHydration();
  const state = hydration.merge(head(), agent, false);
  const pending = frame({
    ...fixture.live_requested.data,
    sequence: 11,
    cursor: raw.through_cursor.replace(/\.9$/, '.11'),
    kind: { ...fixture.live_requested.data.kind, revision: 3 },
  });
  expect(hydration.applyV2(state, pending, agent, true).approvals).toEqual([]);
  const resolved = foldV2Item(snapshot(), v2AgentEvent(fixture.live_resolved.data, true));
  expect(hydration.merge(resolved, agent, true).approvals).toMatchObject([
    { decision: 'approved' },
  ]);
});

it('applies a recorded terminal immediately while waiting for history and retains it on hydration', () => {
  const hydration = new ConversationHydration();
  const state = hydration.merge(head(), agent, false);
  const ended = hydration.applyV2(state, frame(fixture.live_terminal.data), agent, true);
  expect(ended.activeRunId).toBeUndefined();
  expect(ended.turnOutcomes?.['3']).toBe('failed');
  const merged = hydration.merge(snapshot(), agent, true);
  expect(merged.activeRunId).toBeUndefined();
  expect(merged.turnOutcomes?.['3']).toBe('failed');
  expect(
    merged.messages.some((message) => message.content === 'Synthetic retained partial output.'),
  ).toBe(true);
});

it('keeps the strict missing-request and identity boundaries after complete history', () => {
  const hydration = new ConversationHydration();
  const state = hydration.merge(head(), agent, false);
  expect(() =>
    hydration.applyV2(
      state,
      frame({ ...fixture.live_resolved.data, agent_id: 'agt_other' }),
      agent,
      true,
    ),
  ).toThrow('identity');
  expect(() => hydration.applyV2(state, frame(fixture.live_resolved.data), agent, false)).toThrow(
    'retained request',
  );
});
