import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import type { AgentEvent, AgentLiveEvent, AgentTranscript, ResourcePage } from '../api/types';
import { agentClient, AGENT_TRANSCRIPT } from '../test/agent';
import { animateArrival } from '../ui/arrivalMotion';
import { SharedAgent } from './SharedAgent';
import { loadSharedAgentAccess, loadSharedAgentHead } from './loadSharedAgent';
import type { ResourceViewsClient } from './client';

vi.mock('../ui/arrivalMotion', () => ({ animateArrival: vi.fn() }));
afterEach(() => vi.restoreAllMocks());

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

const incarnation = 'a'.repeat(64);
const head = {
  object: 'cma.agent.event_head',
  schema_version: 1,
  agent_id: 'agt_1',
  incarnation,
  through_cursor: `ag1.${incarnation}.5`,
  active_turn: {
    cursor: `ag1.${incarnation}.5`,
    sequence: 5,
    appended_at: '2026-10-22T06:00:00Z',
    kind: { type: 'turn_started', turn_sequence: 5 },
    response_binding: null,
  },
};
const detail = {
  access: 'shared' as const,
  id: 'agt_1',
  title: 'Shared live investigation',
  status: 'ready' as const,
  created_at: '2026-10-22T05:00:00Z',
  updated_at: '2026-10-22T05:00:00Z',
};
const event = (type: string, fields: Record<string, unknown> = {}): AgentLiveEvent => ({
  type,
  threadId: 'agt_1',
  runId: '5',
  ...fields,
});
const liveReply = event('TEXT_MESSAGE_CONTENT', {
  messageId: 'live-answer',
  delta: 'A reply before history arrives.',
  'cma:live': true,
});
const oldHistory: AgentEvent[] = [
  { type: 'turn_started', turn_sequence: 1 },
  { type: 'owner_message', content: 'Inspect this repository.' },
  { type: 'agent_message', content: 'The repository is ready.' },
  { type: 'turn_completed', turn_sequence: 1 },
].map((kind, index) => ({ seq: index + 1, appendedAt: detail.created_at, kind }));

type Head = Omit<typeof head, 'active_turn'> & { active_turn: typeof head.active_turn | null };
function fixture({
  heldHead = false,
  supportsHead = true,
  headValue = head,
}: {
  heldHead?: boolean;
  supportsHead?: boolean;
  headValue?: Head;
} = {}) {
  const transcript = deferred<AgentTranscript>();
  const activity = deferred<ResourcePage<AgentEvent, number>>();
  const pendingHead = deferred<Head>();
  const state = { policy: 1 };
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname;
    if (path.includes('/resource-access/'))
      return Response.json({
        capabilities: ['view_details', 'view_history', 'interact'],
        policy_revision: state.policy,
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
      });
    if (path.endsWith('/events/head'))
      return Response.json(heldHead ? await pendingHead.promise : headValue);
    if (path.endsWith('/input')) return Response.json({ queued: false }, { status: 202 });
    if (path.endsWith('/provenance'))
      return Response.json({
        agent_id: 'agt_1',
        created_at: detail.created_at,
        creation_origin: 'manual',
      });
    throw new Error(`Unexpected public route ${path}`);
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const client: ResourceViewsClient = supportsHead
    ? sdk
    : {
        agents: sdk.agents,
        resourceAccess: sdk.resourceAccess,
        sandboxes: sdk.sandboxes,
        workspaces: sdk.workspaces,
      };
  let emit!: (event: AgentLiveEvent) => void;
  const replayAgent = vi.fn<ReturnType<typeof agentClient>['replayAgent']>((_id, options) => {
    emit = options.onEvent;
    return new Promise((resolve) =>
      options.signal?.addEventListener('abort', () => resolve(), { once: true }),
    );
  });
  const readAgentTranscript = vi.fn(() => transcript.promise);
  const readAgentEvents = vi.fn(() => activity.promise);
  const web = agentClient({
    resourceViews: client,
    replayAgent,
    readAgentTranscript,
    readAgentEvents,
  });
  const onAccessChanged = vi.fn();
  const show = () =>
    render(
      <CmaClientProvider client={web}>
        <SharedAgent
          client={client}
          web={web}
          detail={detail}
          onAccessChanged={onAccessChanged}
          detailsOpen={false}
          onDetailsOpenChange={vi.fn()}
        />
      </CmaClientProvider>,
    );
  return {
    show,
    client,
    transcript,
    activity,
    pendingHead,
    state,
    fetch,
    replayAgent,
    readAgentTranscript,
    readAgentEvents,
    onAccessChanged,
    emit: (value: AgentLiveEvent) => emit(value),
  };
}

it('streams Thinking and animated replies while transcript and activity are held, preserving the draft through hydration', async () => {
  const test = fixture();
  test.show();
  await waitFor(() => expect(test.replayAgent).toHaveBeenCalledOnce());
  expect(test.replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 5 }));
  expect(document.querySelector('.active-thinking')).toBeInTheDocument();
  expect(test.readAgentEvents).not.toHaveBeenCalled();
  const user = userEvent.setup();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  await user.type(composer, 'Work before history finishes');
  await user.click(screen.getByRole('button', { name: 'Queue message' }));
  await waitFor(() => expect(composer).toHaveValue(''));
  expect(test.fetch.mock.calls.filter(([url]) => String(url).endsWith('/input'))).toHaveLength(1);
  await user.type(composer, 'Keep my next draft');
  vi.mocked(animateArrival).mockClear();
  await act(async () => test.emit(liveReply));
  expect(screen.getByText('A reply before history arrives.')).toBeInTheDocument();
  expect(animateArrival).toHaveBeenCalled();
  expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
  await act(async () => test.transcript.resolve(AGENT_TRANSCRIPT));
  await screen.findByText('The repository is ready.');
  expect(screen.getAllByText('A reply before history arrives.')).toHaveLength(1);
  expect(document.querySelector('.active-thinking')).toBeInTheDocument();
  await waitFor(() => expect(test.readAgentEvents).toHaveBeenCalledOnce());
  await act(async () => test.activity.resolve({ items: oldHistory }));
  expect(screen.getAllByText('A reply before history arrives.')).toHaveLength(1);
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
  expect(document.querySelector('.active-thinking')).toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep my next draft');
  expect(test.replayAgent).toHaveBeenCalledOnce();
  await act(async () => {
    test.emit(event('TEXT_MESSAGE_START', { messageId: 'msg_7', role: 'assistant' }));
    test.emit(
      event('TEXT_MESSAGE_CONTENT', {
        messageId: 'msg_7',
        delta: 'A reply before history arrives.',
      }),
    );
    test.emit(event('TEXT_MESSAGE_END', { messageId: 'msg_7', cursor: 7 }));
    test.emit(event('RUN_FINISHED', { cursor: 8 }));
  });
  expect(screen.getAllByText('A reply before history arrives.')).toHaveLength(1);
});

it('does not resurrect or duplicate a streamed turn already committed in the late transcript', async () => {
  const test = fixture();
  test.show();
  await waitFor(() => expect(test.replayAgent).toHaveBeenCalledOnce());
  await act(async () => test.emit(liveReply));
  const caughtUp: AgentTranscript = {
    ...AGENT_TRANSCRIPT,
    throughSeq: 8,
    turns: [
      ...AGENT_TRANSCRIPT.turns,
      {
        turnSequence: 5,
        startedAt: detail.updated_at,
        ownerMessage: 'Continue',
        agentMessages: ['A reply before history arrives.'],
        outcome: { type: 'completed' },
        messages: [
          {
            sequence: 7,
            role: 'assistant',
            content: 'A reply before history arrives.',
            appendedAt: detail.updated_at,
          },
        ],
      },
    ],
  };
  await act(async () => test.transcript.resolve(caughtUp));
  await screen.findByText('The repository is ready.');
  await waitFor(() => expect(test.readAgentEvents).toHaveBeenCalledOnce());
  await act(async () => {
    test.emit(
      event('TEXT_MESSAGE_CONTENT', {
        messageId: 'msg_7',
        delta: 'A reply before history arrives.',
      }),
    );
    test.emit(event('TEXT_MESSAGE_END', { messageId: 'msg_7', cursor: 7 }));
    test.activity.resolve({
      items: [
        ...oldHistory,
        { seq: 5, appendedAt: detail.updated_at, kind: { type: 'turn_started', turn_sequence: 5 } },
        {
          seq: 7,
          appendedAt: detail.updated_at,
          kind: { type: 'agent_message', content: 'A reply before history arrives.' },
        },
        {
          seq: 8,
          appendedAt: detail.updated_at,
          kind: { type: 'turn_completed', turn_sequence: 5 },
        },
      ],
    });
  });
  expect(screen.getAllByText('A reply before history arrives.')).toHaveLength(1);
  await waitFor(() => expect(document.querySelector('.active-thinking')).not.toBeInTheDocument());
});

it('keeps a transcript-started stream when the head finishes later', async () => {
  const test = fixture({ heldHead: true });
  test.show();
  await screen.findByRole('textbox', { name: 'Message' });
  await act(async () => test.transcript.resolve(AGENT_TRANSCRIPT));
  await waitFor(() => expect(test.replayAgent).toHaveBeenCalledOnce());
  expect(test.replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 4 }));
  await act(async () => {
    test.emit(event('RUN_STARTED', { cursor: 5 }));
    test.emit(liveReply);
    test.pendingHead.resolve(head);
  });
  expect(screen.getAllByText('A reply before history arrives.')).toHaveLength(1);
  expect(test.replayAgent).toHaveBeenCalledOnce();
});

it('keeps the transcript boundary after activity fails so late old replies cannot reappear', async () => {
  const test = fixture({
    headValue: {
      ...head,
      through_cursor: `ag1.${incarnation}.2`,
      active_turn: {
        ...head.active_turn,
        cursor: `ag1.${incarnation}.1`,
        sequence: 1,
        kind: { type: 'turn_started', turn_sequence: 1 },
      },
    },
  });
  test.readAgentEvents.mockRejectedValueOnce(new Error('Activity temporarily unavailable'));
  test.show();
  await waitFor(() => expect(test.replayAgent).toHaveBeenCalledOnce());
  await act(async () => test.transcript.resolve(AGENT_TRANSCRIPT));
  await screen.findByText(
    'Conversation activity could not be loaded. Your recorded messages are shown.',
  );
  await act(async () => {
    test.emit({ ...liveReply, runId: '1', delta: 'Obsolete reply fragment' });
    test.emit(event('RUN_STARTED', { runId: '1', cursor: 1 }));
  });
  expect(screen.queryByText('Obsolete reply fragment')).not.toBeInTheDocument();
  await waitFor(() => expect(document.querySelector('.active-thinking')).not.toBeInTheDocument());
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
  await act(async () => {
    test.emit(event('RUN_STARTED', { cursor: 5 }));
    test.emit(liveReply);
  });
  expect(screen.getAllByText('A reply before history arrives.')).toHaveLength(1);
  expect(document.querySelector('.active-thinking')).toBeInTheDocument();
  expect(test.replayAgent).toHaveBeenCalledOnce();
});

it('hydrates the gap between an older transcript and the newer idle live baseline', async () => {
  const test = fixture({
    headValue: { ...head, through_cursor: `ag1.${incarnation}.8`, active_turn: null },
  });
  test.show();
  await waitFor(() => expect(test.replayAgent).toHaveBeenCalledOnce());
  expect(test.replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 8 }));
  await act(async () =>
    test.transcript.resolve({
      ...AGENT_TRANSCRIPT,
      turns: AGENT_TRANSCRIPT.turns.map((turn) => ({ ...turn, outcome: { type: 'active' } })),
    }),
  );
  await waitFor(() => expect(test.readAgentEvents).toHaveBeenCalledOnce());
  expect(document.querySelector('.active-thinking')).not.toBeInTheDocument();
  await act(async () =>
    test.activity.resolve({
      items: [
        ...oldHistory,
        { seq: 5, appendedAt: detail.updated_at, kind: { type: 'turn_started', turn_sequence: 5 } },
        {
          seq: 7,
          appendedAt: detail.updated_at,
          kind: { type: 'agent_message', content: 'Recorded between transcript and head.' },
        },
        {
          seq: 8,
          appendedAt: detail.updated_at,
          kind: { type: 'turn_completed', turn_sequence: 5 },
        },
      ],
    }),
  );
  expect(await screen.findByText('Recorded between transcript and head.')).toBeInTheDocument();
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
  expect(document.querySelector('.active-thinking')).not.toBeInTheDocument();
  expect(test.replayAgent).toHaveBeenCalledOnce();
});

it('retains transcript-first replay for a client without the optional head API', async () => {
  const test = fixture({ supportsHead: false });
  test.show();
  await screen.findByRole('textbox', { name: 'Message' });
  expect(test.replayAgent).not.toHaveBeenCalled();
  expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/events/head'))).toBe(false);
  await act(async () => test.transcript.resolve(AGENT_TRANSCRIPT));
  await waitFor(() => expect(test.replayAgent).toHaveBeenCalledOnce());
  expect(test.replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 4 }));
});

it('refuses a head whose shared policy changes before publication', async () => {
  const test = fixture({ heldHead: true });
  test.show();
  await waitFor(() =>
    expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/events/head'))).toBe(true),
  );
  test.state.policy += 1;
  await act(async () => test.pendingHead.resolve(head));
  await waitFor(() => expect(test.onAccessChanged).toHaveBeenCalled());
  expect(test.replayAgent).not.toHaveBeenCalled();
  expect(document.querySelector('.active-thinking')).not.toBeInTheDocument();
});

it('rejects a late head after its original display authority expires', async () => {
  const test = fixture({ heldHead: true });
  const signal = new AbortController().signal;
  const authorization = await loadSharedAgentAccess(test.client, 'agt_1', signal);
  const pending = loadSharedAgentHead(test.client, authorization.value, signal);
  await waitFor(() =>
    expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/events/head'))).toBe(true),
  );
  vi.spyOn(Date, 'now').mockReturnValue(authorization.value.deadline + 1);
  test.pendingHead.resolve(head);
  await expect(pending).rejects.toThrow('Agent access changed');
});
