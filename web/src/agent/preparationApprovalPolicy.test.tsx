import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { Dispatch, SetStateAction } from 'react';
import fixture from '../test/externalApprovalProjection.fixture.json';
import { AGENT_DETAIL } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript } from './conversation';
import type { AgentResourceState } from './controllerTypes';
import type { ConversationState } from './model';
import { MessageStream } from './MessageStream';
import { usePreparationProgress } from './preparationProgress';
import { ReadinessWake } from './readinessWait';
import { harness, cursor } from './transport/controller.fixture';
import { deferred } from './transport/load.test.fixture';
import { foldV2Item, foldV2Frame } from './transport/projection';
import { withApprovalProjection } from './transport/externalApprovalProjection';
import { observeApprovalProjection } from './transport/approvalProjectionObserver';
import { v2AgentEvent } from '../api-client/agent-events-v2/types';

beforeEach(() => sessionStorage.clear());
const raw = { ...fixture.waiting_snapshot, agent_id: 'agt_1' };
const headers = fixture.response_headers;
const unsupported = { supported: false, canReview: false };

async function setup(initiallySupported = true) {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const original = session.journal.retain('Original unresolved input');
  h.snapshot.mockResolvedValueOnce(
    Response.json(
      initiallySupported
        ? raw
        : {
            ...raw,
            items: raw.items.slice(0, 8),
            through_cursor: cursor(8),
          },
      { headers: initiallySupported ? headers : {} },
    ),
  );
  const baseline = await session.events.snapshot('agt_1', { externalApprovals: true });
  const conversation = baseline.items.reduce<ConversationState>(
    foldV2Item,
    withApprovalProjection(
      {
        ...conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
        agentIncarnation: raw.incarnation,
      },
      baseline.externalApprovalProjection ?? unsupported,
    ),
  );
  const state: { current: AgentResourceState } = {
    current: {
      status: 'ready',
      detail: { ...AGENT_DETAIL, status: 'starting', firstMessage: undefined },
      workspace: workspaceFixture(),
      conversation,
      v2: session,
    },
  };
  const publish = vi.fn<Dispatch<SetStateAction<AgentResourceState>>>((update) => {
    state.current = typeof update === 'function' ? update(state.current) : update;
  });
  const pending = deferred<Response>();
  h.snapshot.mockReset().mockReturnValueOnce(pending.promise);
  const errors = vi.fn();
  const liveCursor = { current: conversation.cursor };
  const wake = new ReadinessWake();
  const hook = renderHook(() =>
    usePreparationProgress({
      agentId: 'agt_1',
      client: h.client,
      provisioning: true,
      stateRef: state,
      setState: publish,
      setActionError: errors,
      liveCursor,
      hierarchyChanged: { current: undefined },
      onIdentityChange: () => false,
      wake,
    }),
  );
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
  const complete = async (responseHeaders: HeadersInit = headers) => {
    await act(async () => pending.resolve(Response.json(raw, { headers: responseHeaders })));
    await waitFor(() => {
      expect(state.current.status).toBe('ready');
      if (state.current.status === 'ready') expect(state.current.v2).not.toBe(session);
    });
    hook.unmount();
    if (state.current.status !== 'ready') throw new Error('Expected refreshed ready state');
    expect(state.current.v2?.journal).toBe(session.journal);
    expect(state.current.v2?.journal.read()).toEqual(original);
    expect(h.post).not.toHaveBeenCalled();
    expect(h.control).not.toHaveBeenCalled();
    expect(h.client.runAgent).not.toHaveBeenCalled();
    // No SSE handshake is available to repair the published policy in these cases.
    expect(h.streaming).toBe(false);
    return state.current;
  };
  return { ...h, state, publish, session, conversation, complete, liveCursor };
}

it.each([
  ['read-only action', { ...headers, 'CMA-External-Approval-Action': 'none' }],
  [
    'rotated review origin',
    { ...headers, 'CMA-Approval-Review-Origin': 'https://replacement.example.test' },
  ],
  ['missing review origin', { 'Preference-Applied': 'cma-external-approval-v1' }],
] as const)(
  'removes old link authority when preparation refresh has %s and an unchanged cursor',
  async (_, nextHeaders) => {
    const h = await setup();
    const view = render(<MessageStream conversation={h.conversation} onApproval={vi.fn()} />);
    expect(screen.getByRole('link', { name: 'Review and approve' })).toBeVisible();
    const refreshed = await h.complete(nextHeaders);
    view.rerender(<MessageStream conversation={refreshed.conversation} onApproval={vi.fn()} />);
    expect(refreshed.conversation.cursor).toBe(h.conversation.cursor);
    expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
    expect(screen.queryByRole('link', { name: 'Review and approve' })).toBeNull();
    // A late callback from the replaced session cannot reinstall its former policy.
    observeApprovalProjection(
      h.session,
      'agt_1',
      new AbortController().signal,
      h.state,
      h.publish,
    )(h.conversation.externalApprovalProjection!);
    expect(h.state.current).toBe(refreshed);
  },
);

it('uses fresh support before folding the first external receipt onto an older unsupported view', async () => {
  const h = await setup(false);
  const refreshed = await h.complete();
  render(<MessageStream conversation={refreshed.conversation} onApproval={vi.fn()} />);
  expect(screen.getByRole('link', { name: 'Review and approve' })).toHaveAttribute(
    'href',
    fixture.live_requested.data.kind.approve_url,
  );
  expect(refreshed.conversation.responseBinding).toEqual({
    commandId: 'cmd_approval_fixture',
    responseId: 'resp_approval_fixture',
    turnSequence: 3,
  });
});

it('retains a newer terminal observed while the preparation snapshot was pending', async () => {
  const h = await setup();
  if (h.state.current.status !== 'ready') throw new Error('Expected ready');
  const ended = foldV2Item(h.conversation, v2AgentEvent(fixture.live_terminal.data, true));
  h.state.current = { ...h.state.current, conversation: ended };
  const refreshed = await h.complete({ ...headers, 'CMA-External-Approval-Action': 'none' });
  render(<MessageStream conversation={refreshed.conversation} onApproval={vi.fn()} />);
  expect(refreshed.conversation.cursor).toBe(10);
  expect(refreshed.conversation.activeRunId).toBeUndefined();
  expect(refreshed.conversation.turnOutcomes?.['3']).toBe('failed');
  expect(screen.getByText('The approval wait timed out. This turn has ended.')).toBeVisible();
  expect(screen.getByText('Synthetic retained partial output.')).toBeVisible();
  expect(screen.queryByRole('link', { name: 'Review and approve' })).toBeNull();
});

it('retains uncommitted live output at the same cursor while replacing policy', async () => {
  const h = await setup();
  if (h.state.current.status !== 'ready') throw new Error('Expected ready');
  const live = foldV2Frame(
    h.conversation,
    {
      type: 'cma.agent.live',
      data: {
        agent_id: 'agt_1',
        incarnation: raw.incarnation,
        turn_sequence: 3,
        after_sequence: 9,
        message_id: 'new_partial',
        kind: 'text_delta',
        delta: 'New live output',
      },
    },
    'agt_1',
  );
  h.state.current = { ...h.state.current, conversation: live };
  const refreshed = await h.complete({ ...headers, 'CMA-External-Approval-Action': 'none' });
  expect(refreshed.conversation.messages).toEqual(live.messages);
  expect(
    refreshed.conversation.messages.some(
      (message) => message.liveOnly && message.content === 'New live output',
    ),
  ).toBe(true);
  expect(refreshed.conversation.externalApprovalReviewPolicy).toBeUndefined();
});
