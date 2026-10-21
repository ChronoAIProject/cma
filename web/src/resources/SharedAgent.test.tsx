import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { Console } from '../Console';
import { agentClient, AGENT_TRANSCRIPT } from '../test/agent';
import { hydrateSharedAgent, loadSharedAgent } from './loadSharedAgent';
import { WIRE_AGENT } from '../test/rest';
import type { AgentEvent, AgentLiveEvent, ResourcePage } from '../api/types';

afterEach(() => vi.useRealTimers());

const detail = {
  access: 'shared',
  id: 'agt_1',
  title: 'Shared investigation',
  status: 'ready',
  created_at: '2026-09-01T00:00:00Z',
  updated_at: '2026-09-01T00:00:00Z',
};

function fixture(
  initial = ['view_details', 'view_history'],
  overrides: Parameters<typeof agentClient>[0] = {},
) {
  const state = { capabilities: initial, revoked: false, policy: 1 };
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    if (state.revoked)
      return Response.json(
        { code: 'resource_not_found', detail: 'Access removed' },
        { status: 404 },
      );
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        capabilities: state.capabilities,
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
        policy_revision: state.policy,
        shared_by: { subject: 'owner-alex', display_name: 'Alex Chen', avatar_url: null },
      });
    if (url.pathname.endsWith('/channel-usage'))
      return Response.json({
        agent_id: 'agt_1',
        channel_usage: {
          active: false,
          has_history: false,
          first_used_at: null,
          last_used_at: null,
        },
      });
    if (url.pathname.endsWith('/provenance'))
      return Response.json({
        agent_id: 'agt_1',
        created_at: detail.created_at,
        creation_origin: 'manual',
      });
    if (url.pathname.endsWith('/input')) {
      expect(init?.method).toBe('POST');
      expect(new Headers(init?.headers).get('idempotency-key')).toBeTruthy();
      return Response.json({ queued: false }, { status: 202 });
    }
    return Response.json(detail);
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const readAgent = vi.fn();
  const readWorkspace = vi.fn();
  const readSandbox = vi.fn();
  const readAgentResources = vi.fn();
  const web = agentClient({
    resourceViews: sdk,
    listSandboxes: async () => ({ items: [] }),
    readAgent,
    readWorkspace,
    readSandbox,
    readAgentResources,
    readAgentProvenance: (id, options) => sdk.agents.provenance(id, options),
    ...overrides,
  });
  const show = () =>
    render(
      <CmaClientProvider client={web}>
        <Console
          user={{
            id: 'reader',
            handle: 'Reader',
            visitor: false,
            capabilities: ['use_own_sandboxes'],
          }}
          route={{ kind: 'agent', agentId: 'agt_1' }}
          onNavigate={vi.fn()}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
  return {
    state,
    sdk,
    web,
    show,
    fetch,
    readAgent,
    readWorkspace,
    readSandbox,
    readAgentResources,
  };
}

it('opens a narrow shared deep link without private ancestry, runtime or personal reads', async () => {
  const test = fixture();
  test.show();
  expect(await screen.findByRole('heading', { name: detail.title })).toBeInTheDocument();
  expect(
    await screen.findByText(/Your current access does not allow interaction/),
  ).toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(screen.getByRole('note')).toHaveTextContent('Shared by Alex Chen');
  expect(screen.getByRole('note')).toHaveTextContent('View-only access');
  expect(screen.queryByRole('button', { name: 'Permission mode' })).not.toBeInTheDocument();
  for (const read of [
    test.readAgent,
    test.readWorkspace,
    test.readSandbox,
    test.readAgentResources,
  ])
    expect(read).not.toHaveBeenCalled();
  await userEvent.setup().click(screen.getByRole('button', { name: 'More Agent actions' }));
  expect(await within(screen.getByRole('menu')).findByText('Created manually')).toBeInTheDocument();
});

it('allows a writable shared Agent to change settings and checks current access before each update', async () => {
  const defaults = {
    model: 'gpt-6',
    reasoning_effort: 'high',
    permission_mode: 'autonomous',
  } as const;
  const update = vi.fn(async (_id, body) => body);
  const test = fixture(['view_details', 'view_history', 'interact', 'operate_agent'], {
    readAgentSettings: vi.fn(async () => defaults),
    readAgentModels: vi.fn(async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }]),
    updateAgentSettings: update,
  });
  test.show();
  const user = userEvent.setup();
  await screen.findByRole('button', { name: 'Permission mode' });
  expect(screen.getByRole('note')).toHaveTextContent('View and write access');
  await user.click(screen.getByRole('button', { name: 'Permission mode' }));
  await user.click(screen.getByRole('menuitemradio', { name: 'Guarded' }));
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText('Settings applied to this Agent.');
  expect(update).toHaveBeenCalledOnce();
  expect(update.mock.calls[0]?.[1]).toMatchObject({ permission_mode: 'guarded' });
  await user.click(screen.getByRole('button', { name: 'Permission mode' }));
  await user.click(screen.getByRole('menuitemradio', { name: 'Autonomous' }));
  test.state.capabilities = ['view_details', 'view_history'];
  await user.click(screen.getByRole('button', { name: 'Apply settings' }));
  await screen.findByText(/Your current access does not allow interaction/);
  expect(update).toHaveBeenCalledOnce();
  expect(screen.queryByRole('button', { name: 'Permission mode' })).not.toBeInTheDocument();
});

it('allows current admin interaction through the public SDK and removes it on downgrade', async () => {
  const test = fixture(['view_details', 'view_history', 'interact', 'resolve_approval']);
  test.show();
  const user = userEvent.setup();
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await user.type(composer, 'Review as the current admin');
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() =>
    expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/input'))).toBe(true),
  );
  expect(test.fetch.mock.calls.filter(([url]) => String(url).endsWith('/input'))).toHaveLength(1);
  test.state.capabilities = ['view_details', 'view_history'];
  await user.click(screen.getByRole('button', { name: 'More Agent actions' }));
  await user.click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  await user.click(await screen.findByRole('button', { name: 'Refresh access' }));
  expect(
    await screen.findByText(/Your current access does not allow interaction/),
  ).toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
});

it('clears a revoked shared page and aborts its open stream without signing out', async () => {
  const test = fixture();
  const streams: AbortSignal[] = [];
  test.web.replayAgent = async (_id, options) =>
    new Promise<void>((resolve) => {
      streams.push(options.signal!);
      options.signal?.addEventListener('abort', () => resolve(), { once: true });
    });
  test.show();
  await screen.findByText(/Your current access does not allow interaction/);
  await waitFor(() => expect(streams.length).toBe(1));
  test.state.revoked = true;
  await userEvent.setup().click(screen.getByRole('button', { name: 'More Agent actions' }));
  await userEvent.setup().click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Refresh access' }));
  expect(await screen.findByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
  expect(screen.queryByText(detail.title)).not.toBeInTheDocument();
  expect(streams.every((signal) => signal.aborted)).toBe(true);
  expect(screen.getByRole('button', { name: 'Account menu for Reader' })).toBeInTheDocument();
});

it('discards history buffered across a policy change', async () => {
  const test = fixture();
  test.web.readAgentTranscript = async () => {
    test.state.policy += 1;
    return AGENT_TRANSCRIPT;
  };
  await expect(
    loadSharedAgent(test.web, test.sdk, 'agt_1', new AbortController().signal),
  ).rejects.toThrow('Agent access changed');
});

const sharedHistoryEvents: AgentEvent[] = [
  { type: 'turn_started', turn_sequence: 1 },
  { type: 'owner_message', content: 'Inspect this repository.' },
  { type: 'tool_call_started', call_id: 'read', title: 'Read files' },
  { type: 'approval_requested', approval_id: 'approval', action: 'Write files' },
  { type: 'approval_resolved', approval_id: 'approval', decision: 'allow' },
  { type: 'tool_call_updated', call_id: 'read', status: 'completed' },
  { type: 'agent_message', content: 'The repository is ready.' },
  { type: 'turn_completed', turn_sequence: 1 },
].map((kind, index) => ({ seq: index + 1, appendedAt: detail.created_at, kind }));

const sharedHistoryTranscript = {
  ...AGENT_TRANSCRIPT,
  throughSeq: 8,
  turns: AGENT_TRANSCRIPT.turns.map((turn) => ({
    ...turn,
    messages: turn.messages.map((message) => ({
      ...message,
      sequence: message.role === 'assistant' ? 7 : 2,
    })),
  })),
};

it('shows the shared transcript tail while rich history rebuilds without historical Thinking or actions', async () => {
  vi.useFakeTimers();
  let finish!: (page: ResourcePage<AgentEvent, number>) => void;
  const remaining = new Promise<ResourcePage<AgentEvent, number>>((resolve) => {
    finish = resolve;
  });
  const readAgentEvents = vi
    .fn()
    .mockResolvedValueOnce({ items: sharedHistoryEvents.slice(0, 4), next: 4 })
    .mockReturnValueOnce(remaining);
  const replayAgent = vi.fn(() => new Promise<void>(() => {}));
  const test = fixture(['view_details', 'view_history', 'interact', 'resolve_approval'], {
    readAgentTranscript: async () => sharedHistoryTranscript,
    readAgentEvents,
    replayAgent,
  });
  const shown = test.show();
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  expect(readAgentEvents).toHaveBeenCalledTimes(2);
  expect(document.querySelector('.active-thinking')).not.toBeInTheDocument();
  expect(screen.queryByText('Read files')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Allow' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(replayAgent).not.toHaveBeenCalled();
  expect(readAgentEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 4]);
  // Rich history is independent of the bounded initial authorization read.
  await act(() => vi.advanceTimersByTimeAsync(16_000));
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  expect(
    screen.queryByText(/Current resource access could not be checked/),
  ).not.toBeInTheDocument();
  await act(async () => finish({ items: sharedHistoryEvents.slice(4) }));
  expect(replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 8 }));
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
  expect(document.querySelector('.active-thinking, .is-arriving')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Allow' })).not.toBeInTheDocument();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  const refresh = vi
    .spyOn(test.web, 'readAgentTranscript')
    .mockImplementationOnce(() => new Promise(() => {}));
  await act(() => vi.advanceTimersByTimeAsync(14_000));
  expect(refresh).toHaveBeenCalledOnce();
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  shown.unmount();
});

it('preserves a new turn admitted after the transcript snapshot when selecting the replay cursor', async () => {
  const test = fixture();
  test.web.readAgentTranscript = async () => sharedHistoryTranscript;
  const snapshot = await loadSharedAgent(test.web, test.sdk, 'agt_1', new AbortController().signal);
  test.web.readAgentEvents = async () => ({
    items: [
      ...sharedHistoryEvents,
      { seq: 9, appendedAt: detail.created_at, kind: { type: 'turn_started', turn_sequence: 9 } },
      {
        seq: 10,
        appendedAt: detail.created_at,
        kind: { type: 'owner_message', content: 'Continue.' },
      },
    ],
  });
  const conversation = await hydrateSharedAgent(
    test.web,
    test.sdk,
    snapshot.value,
    new AbortController().signal,
  );
  expect(conversation.cursor).toBe(10);
  expect(conversation.activeRunId).toBe('9');
  expect(conversation.messages.map((message) => message.content)).toEqual([
    'Inspect this repository.',
    'The repository is ready.',
    'Continue.',
  ]);
});

it('applies shared live events only for the current Agent after rich history is ready', async () => {
  let onEvent: ((event: AgentLiveEvent) => void) | undefined;
  const test = fixture(['view_details', 'view_history'], {
    replayAgent: (_id, options) => {
      onEvent = options.onEvent;
      return new Promise(() => {});
    },
  });
  test.show();
  await waitFor(() => expect(onEvent).toBeDefined());
  await act(async () => {
    onEvent?.({ type: 'RUN_STARTED', runId: '5', threadId: 'agt_1', cursor: 5 });
    onEvent?.({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '5',
      threadId: 'agt_foreign',
      messageId: 'foreign',
      delta: 'A different Agent',
      'cma:live': true,
    });
    onEvent?.({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '5',
      threadId: 'agt_1',
      messageId: 'shared-live',
      delta: 'A live shared answer',
      'cma:live': true,
    });
  });
  expect(screen.getByText('A live shared answer')).toBeInTheDocument();
  expect(screen.queryByText('A different Agent')).not.toBeInTheDocument();
  expect(document.querySelector('.active-thinking')).toBeInTheDocument();
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
});

it('discards rich history after policy change and rejects expired or aborted hydration', async () => {
  const test = fixture();
  const signal = new AbortController().signal;
  const snapshot = await loadSharedAgent(test.web, test.sdk, 'agt_1', signal);
  test.web.readAgentEvents = vi.fn(async () => {
    test.state.policy += 1;
    return { items: sharedHistoryEvents };
  });
  await expect(hydrateSharedAgent(test.web, test.sdk, snapshot.value, signal)).rejects.toThrow(
    'Agent access changed',
  );
  const events = vi.spyOn(test.web, 'readAgentEvents');
  events.mockClear();
  await expect(
    hydrateSharedAgent(test.web, test.sdk, { ...snapshot.value, deadline: Date.now() - 1 }, signal),
  ).rejects.toThrow('Agent access changed');
  const aborted = new AbortController();
  aborted.abort();
  await expect(
    hydrateSharedAgent(test.web, test.sdk, snapshot.value, aborted.signal),
  ).rejects.toThrow();
  expect(events).not.toHaveBeenCalled();
});

it('clears the visible transcript when access is revoked during rich history hydration', async () => {
  let finish!: (page: ResourcePage<AgentEvent, number>) => void;
  let pending: AbortSignal | undefined;
  const replayAgent = vi.fn(() => new Promise<void>(() => {}));
  const test = fixture(['view_details', 'view_history'], {
    readAgentEvents: (_id, options) => {
      pending = options?.signal;
      return new Promise((resolve) => {
        finish = resolve;
      });
    },
    replayAgent,
  });
  test.show();
  await screen.findByText('The repository is ready.');
  await waitFor(() => expect(pending).toBeDefined());
  test.state.revoked = true;
  await act(async () => finish({ items: sharedHistoryEvents }));
  await screen.findByRole('heading', { name: 'Agent unavailable' });
  expect(screen.queryByText('The repository is ready.')).not.toBeInTheDocument();
  expect(pending?.aborted).toBe(true);
  expect(replayAgent).not.toHaveBeenCalled();
});

it('clears the shared header after an initial history refusal while rechecking access', async () => {
  const test = fixture();
  let refuse: ((error: Error) => void) | undefined;
  let finish: ((response: Response) => void) | undefined;
  test.web.readAgentTranscript = () =>
    new Promise((_resolve, reject) => {
      refuse = reject;
    });
  test.show();
  await screen.findByRole('heading', { name: detail.title });
  await waitFor(() => expect(refuse).toBeDefined());
  test.fetch.mockImplementation(
    () =>
      new Promise<Response>((resolve) => {
        finish = resolve;
      }),
  );
  await act(async () => refuse?.(new ApiError(404, 'Access removed', 'resource_not_found')));
  await waitFor(() => expect(finish).toBeDefined());
  expect(screen.queryByText(detail.title)).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  await act(async () =>
    finish?.(
      Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 }),
    ),
  );
  expect(await screen.findByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
  expect(test.readAgent).not.toHaveBeenCalled();
});

it('rejects nonadvancing history and mismatched Agent identities', async () => {
  const test = fixture();
  test.web.readAgentEvents = async () => ({ items: [], next: 0 });
  const snapshot = await loadSharedAgent(test.web, test.sdk, 'agt_1', new AbortController().signal);
  await expect(
    hydrateSharedAgent(test.web, test.sdk, snapshot.value, new AbortController().signal),
  ).rejects.toThrow('pagination did not advance');
  test.web.readAgentTranscript = async () => ({ ...AGENT_TRANSCRIPT, agentId: 'agt_foreign' });
  await expect(
    loadSharedAgent(test.web, test.sdk, 'agt_1', new AbortController().signal),
  ).rejects.toThrow('identity changed');
});

it('shows pending approval history without approval action buttons for a viewer', async () => {
  const test = fixture();
  test.web.readAgentEvents = async () => ({
    items: [
      {
        seq: 5,
        appendedAt: detail.created_at,
        kind: {
          type: 'approval_requested',
          approval_id: 'apr_shared',
          action: 'Review tool execution',
        },
      },
    ],
  });
  test.show();
  expect(
    await screen.findByText('Waiting for an authorized participant to answer this approval.'),
  ).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Allow' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Deny' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Abort' })).not.toBeInTheDocument();
});

it('preserves a personal draft during foreground access refresh and clears the page on refusal', async () => {
  let hold = false;
  let finish: ((response: Response) => void) | undefined;
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (path === '/api/v1/organizations') return Response.json({ items: [] });
      if (path.endsWith('/channel-usage'))
        return Response.json({
          agent_id: 'agt_1',
          channel_usage: {
            active: false,
            has_history: false,
            first_used_at: null,
            last_used_at: null,
          },
        });
      if (hold)
        return new Promise<Response>((resolve) => {
          finish = resolve;
        });
      return Response.json({ ...WIRE_AGENT, id: 'agt_1' });
    },
  });
  const web = agentClient({ resourceViews: sdk });
  render(
    <CmaClientProvider client={web}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await userEvent.setup().type(composer, 'Preserve my personal draft');
  hold = true;
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(finish).toBeDefined());
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Preserve my personal draft');
  await act(async () =>
    finish?.(
      Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 }),
    ),
  );
  expect(await screen.findByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  finish = undefined;
  await userEvent.setup().click(screen.getByRole('button', { name: 'Check access again' }));
  await waitFor(() => expect(finish).toBeDefined());
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Opening Agent' })).toBeInTheDocument();
});

it('discards personal classification as soon as a shared response arrives and keeps it discarded during refresh', async () => {
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }),
  });
  const web = agentClient({ resourceViews: sdk });
  render(
    <CmaClientProvider client={web}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await screen.findByRole('textbox', { name: 'Message' });
  const current = fixture();
  vi.spyOn(sdk.agents, 'readAccessible').mockImplementation((...args) =>
    current.sdk.agents.readAccessible(...args),
  );
  let finish:
    ((value: Awaited<ReturnType<typeof sdk.resourceAccess.effective>>) => void) | undefined;
  const effective = vi.spyOn(sdk.resourceAccess, 'effective').mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(finish).toBeDefined());
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  effective.mockImplementation((...args) => current.sdk.resourceAccess.effective(...args));
  await act(async () =>
    finish?.({
      capabilities: ['view_details', 'view_history'],
      revalidate_at: null,
      policy_revision: 1,
    }),
  );
  await screen.findByText(/Your current access does not allow interaction/);
  vi.spyOn(sdk.agents, 'readAccessible').mockImplementation(() => new Promise(() => {}));
  await userEvent.setup().click(screen.getByRole('button', { name: 'More Agent actions' }));
  await userEvent.setup().click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Refresh access' }));
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Opening Agent' })).toBeInTheDocument();
});

it('uses the personal details drawer without private reads and clears details after revocation', async () => {
  const test = fixture();
  test.show();
  const user = userEvent.setup();
  const toggle = await screen.findByRole('button', { name: 'More Agent actions' });
  expect(screen.queryByRole('complementary', { name: 'Agent details' })).not.toBeInTheDocument();
  expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/channel-usage'))).toBe(false);
  await user.click(toggle);
  await user.click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  const panel = await screen.findByRole('complementary', { name: 'Agent details' });
  expect(await within(panel).findByText(detail.title)).toBeInTheDocument();
  expect(await within(panel).findByText('No channel activity recorded')).toBeInTheDocument();
  expect(within(panel).queryByRole('button', { name: 'Artifacts' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Close Agent details' }));
  expect(toggle).toHaveFocus();
  await user.click(toggle);
  await user.click(screen.getByRole('menuitem', { name: 'Show Agent details' }));
  expect(
    await within(screen.getByRole('complementary', { name: 'Agent details' })).findByText(
      detail.title,
    ),
  ).toBeInTheDocument();
  test.state.revoked = true;
  act(() => window.dispatchEvent(new Event('focus')));
  expect(await screen.findByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
  expect(screen.queryByText(detail.title)).not.toBeInTheDocument();
  for (const read of [
    test.readAgent,
    test.readWorkspace,
    test.readSandbox,
    test.readAgentResources,
  ])
    expect(read).not.toHaveBeenCalled();
});

it.each([
  ['2026-08-31T23:59:00Z', 'Ready'],
  ['2026-09-01T00:01:00Z', 'Paused'],
])('uses timestamped shared status instead of stale replay (%s)', async (appendedAt, label) => {
  const test = fixture();
  test.web.readAgentEvents = async () => ({
    items: [
      {
        seq: 5,
        appendedAt,
        kind: { type: 'status_changed', to: 'paused' },
      },
    ],
  });
  test.show();
  await screen.findByText(/Your current access does not allow interaction/);
  await waitFor(() =>
    expect(document.querySelector('.agent-header .badge')).toHaveTextContent(label),
  );
});
