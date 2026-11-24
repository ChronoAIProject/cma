import { beforeEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../../test/agent';
import { workspaceFixture } from '../../test/hierarchy';
import { conversationFromTranscript } from '../conversation';
import type { AgentResourceState } from '../controllerTypes';
import { harness } from './controller.fixture';
import { approvalProjectionView, observeApprovalProjection } from './approvalProjectionObserver';
import { withApprovalProjection } from './externalApprovalProjection';

beforeEach(() => sessionStorage.clear());
const policy = { supported: true, canReview: true, reviewOrigin: 'https://identity.example.test' };
it('removes stale policy even if buffered history arrives later, without altering pending input', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const pending = session.journal.retain('Original unresolved input');
  const state: AgentResourceState = {
    status: 'ready',
    detail: AGENT_DETAIL,
    workspace: workspaceFixture(),
    v2: session,
    conversation: withApprovalProjection(
      conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
      policy,
    ),
  };
  const publish = vi.fn();
  const observe = observeApprovalProjection(
    session,
    'agt_1',
    new AbortController().signal,
    { current: state },
    publish,
  );
  expect(() => observe({ supported: false, canReview: false })).toThrow('Reload this Agent');
  expect(session.approvalProjectionAvailable).toBe(false);
  const delayed = approvalProjectionView(state);
  if (delayed.status !== 'ready') throw new Error('Expected retained history');
  expect(delayed.conversation.externalApprovalReviewPolicy).toBeUndefined();
  expect(() => observe(policy)).toThrow('Reload this Agent');
  expect(session.journal.read()).toEqual(pending);
  const fresh = session.forSnapshotRefresh();
  expect(fresh.approvalProjectionAvailable).toBe(true);
  expect(fresh.journal.read()).toEqual(pending);
  expect(h.post).not.toHaveBeenCalled();
});

it('does not attach late policy to an aborted view or another Agent', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const state: AgentResourceState = {
    status: 'ready',
    detail: AGENT_DETAIL,
    workspace: workspaceFixture(),
    v2: session,
    conversation: conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
  };
  const publish = vi.fn();
  const abort = new AbortController();
  abort.abort();
  observeApprovalProjection(session, 'agt_1', abort.signal, { current: state }, publish)(policy);
  observeApprovalProjection(
    session,
    'agt_other',
    new AbortController().signal,
    { current: state },
    publish,
  )(policy);
  expect(publish).not.toHaveBeenCalled();
});
