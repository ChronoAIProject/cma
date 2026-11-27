import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { Console } from '../Console';
import { agentClient, AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { hydrateSharedAgent, loadSharedAgent } from './loadSharedAgent';
import { WIRE_AGENT } from '../test/rest';
import type { AgentEvent, AgentLiveEvent, AgentTranscript, ResourcePage } from '../api/types';

afterEach(() => vi.useRealTimers());

it('retains an unsaved file draft when write access is withdrawn but read access remains', async () => {
  const test = fixture(['view_details', 'view_history', 'view_files', 'operate_agent'], {
    files: {
      list: async () => [
        { path: 'notes.txt', name: 'notes.txt', kind: 'file', size_bytes: 5, hidden: false },
      ],
      read: async () => ({
        path: 'notes.txt',
        size_bytes: 5,
        sha256: 'a'.repeat(64),
        content_base64: btoa('Notes'),
      }),
      download: async () => new Blob(['Notes']),
      write: async () => {
        throw new Error('No save is requested');
      },
      remove: async () => {
        throw new Error('No deletion is requested');
      },
    },
  });
  test.show();
  await userEvent.click(await screen.findByRole('button', { name: 'Show Agent computer' }));
  await userEvent.click(screen.getByRole('tab', { name: 'Files' }));
  await userEvent.click(await screen.findByRole('button', { name: 'notes.txt' }));
  const editor = await screen.findByRole('textbox', { name: 'Contents of notes.txt' });
  fireEvent.change(editor, { target: { value: 'Keep this unsaved draft' } });
  test.state.capabilities = ['view_details', 'view_history', 'view_files'];
  test.state.policy += 1;
  fireEvent.focus(window);
  await waitFor(() =>
    expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument(),
  );
  expect(await screen.findByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
    'Keep this unsaved draft',
  );
  expect(editor).toHaveAttribute('readonly');
});

const detail = {
  access: 'shared',
  id: 'agt_1',
  title: 'Shared investigation',
  status: 'ready',
  created_at: '2026-09-01T00:00:00Z',
  updated_at: '2026-09-01T00:00:00Z',
};

const personalDetail = {
  ...WIRE_AGENT,
  id: AGENT_DETAIL.id,
  workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
  sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
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
  await userEvent.setup().click(screen.getByRole('button', { name: 'Show Agent details' }));
  expect(
    await within(screen.getByRole('complementary', { name: 'Agent details' })).findByText(
      'Created manually',
    ),
  ).toBeInTheDocument();
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
  await user.click(await screen.findByRole('button', { name: /^Agent settings:/ }));
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
  await user.click(screen.getByRole('button', { name: 'Show Agent details' }));
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
  await userEvent.setup().click(screen.getByRole('button', { name: 'Show Agent details' }));
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

it('shares the authorized conversation observation with the computer and clears revoked output', async () => {
  let deliver: ((event: AgentLiveEvent) => void) | undefined;
  const replayAgent = vi.fn(async (_id, options) => {
    deliver = options.onEvent;
    await new Promise<void>((resolve) =>
      options.signal.addEventListener('abort', () => resolve(), { once: true }),
    );
  });
  const test = fixture(['view_details', 'view_history'], {
    readAgentTranscript: async () => sharedHistoryTranscript,
    readAgentEvents: async () => ({
      items: sharedHistoryEvents.map((event) =>
        event.kind.type === 'tool_call_started'
          ? { ...event, kind: { ...event.kind, kind: 'execute', title: 'printf shared-proof' } }
          : event.kind.type === 'tool_call_updated'
            ? {
                ...event,
                kind: { ...event.kind, output: { text: 'shared-proof', truncated: false } },
              }
            : event,
      ),
    }),
    replayAgent,
  });
  test.show();
  await userEvent.click(await screen.findByRole('button', { name: 'Show Agent computer' }));
  await userEvent.click(screen.getByRole('tab', { name: 'Operations' }));
  expect(await screen.findByText('shared-proof', { exact: true })).toBeInTheDocument();
  await userEvent.click(screen.getByRole('tab', { name: 'Screen' }));
  await userEvent.click(screen.getByRole('tab', { name: 'Operations' }));
  expect(replayAgent).toHaveBeenCalledOnce();
  test.state.capabilities = ['view_details'];
  test.state.policy += 1;
  fireEvent.focus(window);
  await waitFor(() =>
    expect(screen.queryByText('shared-proof', { exact: true })).not.toBeInTheDocument(),
  );
  await act(async () =>
    deliver?.({
      type: 'TOOL_CALL_END',
      threadId: 'agt_1',
      runId: '1',
      toolCallId: 'read',
      'cma:output': { text: 'shared-proof', truncated: false },
    }),
  );
  expect(screen.queryByText('shared-proof', { exact: true })).not.toBeInTheDocument();
});

it('does not invent an operation or status from output without a recorded call', async () => {
  let deliver: ((event: AgentLiveEvent) => void) | undefined;
  const fixtureView = fixture(['view_details', 'view_history'], {
    readAgentTranscript: async () => ({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    readAgentEvents: async () => ({ items: [] }),
    replayAgent: async (_agent, options) => {
      deliver = options.onEvent;
      await new Promise<void>((resolve) =>
        options.signal?.addEventListener('abort', () => resolve(), { once: true }),
      );
    },
  });
  fixtureView.show();
  await waitFor(() => expect(deliver).toBeDefined());
  await act(async () =>
    deliver?.({
      type: 'CUSTOM',
      name: 'cma.tool_output_reported',
      threadId: 'agt_1',
      runId: '1',
      value: { toolCallId: 'unobserved-call', output: { text: 'partial proof', truncated: false } },
    }),
  );
  expect(screen.queryByText('Tool call', { exact: true })).not.toBeInTheDocument();
});

it('sends with current access before the shared transcript or rich history arrives', async () => {
  let finish!: (value: typeof AGENT_TRANSCRIPT) => void;
  const transcript = new Promise<typeof AGENT_TRANSCRIPT>((resolve) => {
    finish = resolve;
  });
  const readAgentEvents = vi.fn(() => new Promise<ResourcePage<AgentEvent, number>>(() => {}));
  const readAgentTranscript = vi.fn(() => transcript);
  const test = fixture(['view_details', 'view_history', 'interact'], {
    readAgentTranscript,
    readAgentEvents,
  });
  test.show();
  const user = userEvent.setup();
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await user.type(composer, 'Start while history is loading.');
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(composer).toHaveValue(''));
  expect(test.fetch.mock.calls.filter(([url]) => String(url).endsWith('/input'))).toHaveLength(1);
  expect(readAgentTranscript).toHaveBeenCalledOnce();
  expect(readAgentEvents).not.toHaveBeenCalled();
  await user.type(composer, 'Keep my next draft');
  await act(async () => finish(AGENT_TRANSCRIPT));
  await screen.findByText('The repository is ready.');
  expect(readAgentEvents).toHaveBeenCalledOnce();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep my next draft');
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
});

it('refuses a send after shared access is revoked while history is still pending', async () => {
  let historySignal: AbortSignal | undefined;
  const test = fixture(['view_details', 'view_history', 'interact'], {
    readAgentTranscript: (_id, options) => {
      historySignal = options?.signal;
      return new Promise(() => {});
    },
  });
  test.show();
  const user = userEvent.setup();
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await user.type(composer, 'Do not submit after revocation');
  test.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByRole('heading', { name: 'Agent unavailable' });
  expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/input'))).toBe(false);
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(historySignal?.aborted).toBe(true);
});

it('retains new live messages and resolved controls when older rich history finishes late', async () => {
  let finish!: (page: ResourcePage<AgentEvent, number>) => void;
  let onEvent!: (event: AgentLiveEvent) => void;
  const test = fixture(['view_details', 'view_history', 'interact', 'resolve_approval'], {
    readAgentTranscript: async () => sharedHistoryTranscript,
    readAgentEvents: () => new Promise((resolve) => (finish = resolve)),
    replayAgent: (_id, options) => {
      onEvent = options.onEvent;
      return new Promise(() => {});
    },
  });
  test.show();
  await screen.findByText('The repository is ready.');
  await waitFor(() => expect(onEvent).toBeDefined());
  await act(async () => {
    onEvent({ type: 'RUN_STARTED', runId: '9', threadId: 'agt_1', cursor: 9 });
    onEvent({
      type: 'TEXT_MESSAGE_START',
      runId: '9',
      threadId: 'agt_1',
      cursor: 10,
      messageId: 'msg_10',
      role: 'user',
    });
    onEvent({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '9',
      threadId: 'agt_1',
      messageId: 'msg_10',
      delta: 'A new message during history loading.',
    });
    onEvent({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '9',
      threadId: 'agt_1',
      messageId: 'live-answer',
      delta: 'A new live answer',
      'cma:live': true,
    });
    onEvent({
      type: 'STATE_DELTA',
      runId: '9',
      threadId: 'agt_1',
      cursor: 11,
      delta: [
        {
          op: 'add',
          path: '/pendingApprovals/new-approval',
          value: { id: 'new-approval', reason: 'New approval' },
        },
      ],
    });
    onEvent({
      type: 'STATE_DELTA',
      runId: '9',
      threadId: 'agt_1',
      cursor: 12,
      delta: [{ op: 'remove', path: '/pendingApprovals/new-approval' }],
    });
  });
  expect(screen.getByText('A new live answer')).toBeInTheDocument();
  await act(async () =>
    finish({
      items: [
        ...sharedHistoryEvents,
        { seq: 9, appendedAt: detail.created_at, kind: { type: 'turn_started', turn_sequence: 9 } },
        {
          seq: 10,
          appendedAt: detail.created_at,
          kind: { type: 'owner_message', content: 'A new message during history loading.' },
        },
      ],
    }),
  );
  expect(screen.getAllByText('A new message during history loading.')).toHaveLength(1);
  expect(screen.getAllByText('A new live answer')).toHaveLength(1);
  const process = screen.getByRole('button', { name: /^Read / });
  expect(process).toHaveAttribute('aria-expanded', 'false');
  await userEvent.click(process);
  expect(process).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByRole('article', { name: 'Tool call: Read files' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Allow' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
  await act(async () =>
    onEvent({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '9',
      threadId: 'agt_1',
      messageId: 'live-answer',
      delta: ' continues.',
      'cma:live': true,
    }),
  );
  expect(screen.getByText('A new live answer continues.')).toBeInTheDocument();
});

it('allows sending beside the shared transcript while rich history rebuilds without historical Thinking', async () => {
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
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Continue before older activity arrives.' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(test.fetch.mock.calls.filter(([url]) => String(url).endsWith('/input'))).toHaveLength(1);
  expect(composer).toHaveValue('');
  expect(replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 8 }));
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
  const refresh = vi
    .spyOn(test.web, 'readAgentTranscript')
    .mockImplementationOnce(() => new Promise(() => {}));
  await act(() => vi.advanceTimersByTimeAsync(14_000));
  expect(refresh).not.toHaveBeenCalled();
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

it('retains hydrated tools, approvals, live text and the composer across unchanged access refreshes', async () => {
  vi.useFakeTimers();
  let onEvent: ((event: AgentLiveEvent) => void) | undefined;
  const streams: AbortSignal[] = [];
  const replayAgent = vi.fn((_id, options) => {
    onEvent = options.onEvent;
    streams.push(options.signal);
    return new Promise<void>(() => {});
  });
  const readAgentEvents = vi.fn().mockResolvedValueOnce({
    items: [
      ...sharedHistoryEvents,
      { seq: 9, appendedAt: detail.created_at, kind: { type: 'turn_started', turn_sequence: 9 } },
      {
        seq: 10,
        appendedAt: detail.created_at,
        kind: { type: 'owner_message', content: 'Continue.' },
      },
      {
        seq: 11,
        appendedAt: detail.created_at,
        kind: { type: 'approval_requested', approval_id: 'current', action: 'Current command' },
      },
    ],
  });
  const readAgentTranscript = vi.fn(async () => ({
    ...sharedHistoryTranscript,
    throughSeq: 11,
    turns: [
      ...sharedHistoryTranscript.turns,
      {
        turnSequence: 9,
        startedAt: detail.created_at,
        ownerMessage: 'Continue.',
        agentMessages: [],
        messages: [
          {
            sequence: 10,
            role: 'user' as const,
            content: 'Continue.',
            appendedAt: detail.created_at,
          },
        ],
        outcome: { type: 'active' },
      },
    ],
  }));
  const test = fixture(['view_details', 'view_history', 'interact', 'resolve_approval'], {
    readAgentTranscript,
    readAgentEvents,
    replayAgent,
  });
  const shown = test.show();
  await act(() => vi.advanceTimersByTimeAsync(0));
  await act(async () =>
    onEvent?.({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '9',
      threadId: 'agt_1',
      messageId: 'shared-live',
      delta: 'A live partial answer',
      'cma:live': true,
    }),
  );
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Keep this draft' } });
  const process = screen.getByRole('button', { name: /^Read / });
  expect(process).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(process);
  expect(process).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByRole('article', { name: 'Tool call: Read files' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Allow' })).toBeEnabled();
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Queue message' })).toBeEnabled();
  // Any redundant rich-history reload would remain pending and expose a reset.
  readAgentEvents.mockImplementation(() => new Promise(() => {}));
  await act(() => vi.advanceTimersByTimeAsync(30_000));
  expect(readAgentTranscript).toHaveBeenCalledOnce();
  expect(readAgentEvents).toHaveBeenCalledOnce();
  expect(replayAgent).toHaveBeenCalledOnce();
  expect(streams[0]?.aborted).toBe(false);
  expect(screen.getByText('A live partial answer')).toBeInTheDocument();
  expect(screen.getByRole('article', { name: 'Tool call: Read files' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Allow' })).toBeEnabled();
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Queue message' })).toBeEnabled();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep this draft');
  await act(async () =>
    onEvent?.({
      type: 'TEXT_MESSAGE_CONTENT',
      runId: '9',
      threadId: 'agt_1',
      messageId: 'shared-live',
      delta: ' continues.',
      'cma:live': true,
    }),
  );
  expect(screen.getByText('A live partial answer continues.')).toBeInTheDocument();
  test.state.policy += 1;
  test.state.capabilities = ['view_details', 'view_history'];
  await act(() => vi.advanceTimersByTimeAsync(30_000));
  expect(streams[0]?.aborted).toBe(true);
  expect(screen.queryByText('A live partial answer continues.')).not.toBeInTheDocument();
  expect(screen.queryByRole('article', { name: 'Tool call: Read files' })).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  shown.unmount();
});

it('repairs a partially delivered durable message before reconnecting a cleanly ended stream', async () => {
  vi.useFakeTimers();
  let finish!: () => void;
  const streams: AbortSignal[] = [];
  const replayAgent = vi.fn((_id, options) => {
    streams.push(options.signal);
    if (streams.length === 1) {
      options.onEvent({ type: 'RUN_STARTED', runId: '9', threadId: 'agt_1', cursor: 9 });
      options.onEvent({
        type: 'TEXT_MESSAGE_START',
        runId: '9',
        threadId: 'agt_1',
        cursor: 10,
        messageId: 'msg_10',
        role: 'user',
      });
      options.onEvent({
        type: 'TEXT_MESSAGE_CONTENT',
        runId: '9',
        threadId: 'agt_1',
        cursor: 10,
        messageId: 'msg_10',
        delta: 'Continue after',
      });
    }
    return new Promise<void>((resolve) => {
      if (streams.length === 1) finish = resolve;
    });
  });
  const afterDisconnect: AgentTranscript = {
    ...sharedHistoryTranscript,
    throughSeq: 10,
    turns: [
      ...sharedHistoryTranscript.turns,
      {
        turnSequence: 9,
        startedAt: detail.created_at,
        ownerMessage: 'Continue after the stream ended.',
        agentMessages: [],
        messages: [
          {
            sequence: 10,
            role: 'user',
            content: 'Continue after the stream ended.',
            appendedAt: detail.created_at,
          },
        ],
        outcome: { type: 'active' },
      },
    ],
  };
  const readAgentEvents = vi.fn().mockResolvedValue({ items: sharedHistoryEvents });
  const readAgentTranscript = vi
    .fn()
    .mockResolvedValueOnce(sharedHistoryTranscript)
    .mockResolvedValue(afterDisconnect);
  const test = fixture(['view_details', 'view_history', 'interact'], {
    readAgentTranscript,
    readAgentEvents,
    replayAgent,
  });
  const shown = test.show();
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(replayAgent).toHaveBeenCalledWith('agt_1', expect.objectContaining({ after: 8 }));
  expect(screen.getByText('Continue after')).toBeInTheDocument();
  await act(async () => finish());
  await act(() => vi.advanceTimersByTimeAsync(4_999));
  expect(replayAgent).toHaveBeenCalledOnce();
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(readAgentTranscript).toHaveBeenCalledTimes(2);
  expect(readAgentEvents).toHaveBeenCalledTimes(2);
  expect(replayAgent).toHaveBeenCalledTimes(2);
  expect(replayAgent).toHaveBeenLastCalledWith('agt_1', expect.objectContaining({ after: 10 }));
  expect(streams[0]?.aborted).toBe(true);
  expect(streams[1]?.aborted).toBe(false);
  expect(screen.getByText('Continue after the stream ended.')).toBeInTheDocument();
  expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
  shown.unmount();
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
  expect(replayAgent).toHaveBeenCalledOnce();
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
  test.web.readAgentTranscript = async () => ({ ...AGENT_TRANSCRIPT, throughSeq: 5 });
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
      return Response.json(personalDetail);
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
    fetch: async () => Response.json(personalDetail),
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
  await userEvent.setup().click(screen.getByRole('button', { name: 'Show Agent details' }));
  await userEvent.setup().click(await screen.findByRole('button', { name: 'Refresh access' }));
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Opening Agent' })).toBeInTheDocument();
});

it('uses the personal details drawer without private reads and clears details after revocation', async () => {
  const test = fixture();
  test.show();
  const user = userEvent.setup();
  const toggle = await screen.findByRole('button', { name: 'Show Agent details' });
  expect(screen.queryByRole('complementary', { name: 'Agent details' })).not.toBeInTheDocument();
  expect(test.fetch.mock.calls.some(([url]) => String(url).endsWith('/channel-usage'))).toBe(false);
  await user.click(toggle);
  const panel = await screen.findByRole('complementary', { name: 'Agent details' });
  expect(await within(panel).findByText(detail.title)).toBeInTheDocument();
  expect(await within(panel).findByText('No channel activity recorded')).toBeInTheDocument();
  expect(within(panel).queryByRole('button', { name: 'Artifacts' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Close Agent details' }));
  expect(toggle).toHaveFocus();
  await user.click(toggle);
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
  test.web.readAgentTranscript = async () => ({ ...AGENT_TRANSCRIPT, throughSeq: 5 });
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
  expect(await within(screen.getByRole('banner')).findByText(label, { exact: true })).toBeVisible();
});
