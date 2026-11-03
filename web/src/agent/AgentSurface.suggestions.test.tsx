import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { accepted, harness, item, json, page } from './transport/controller.fixture';

beforeEach(() => {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-11-03T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
});
afterEach(() => vi.restoreAllMocks());

async function setup(legacy = false) {
  const fixture = harness();
  if (legacy) {
    fixture.eligibility.mockResolvedValue(
      json({
        object: 'cma.ui.transport',
        schema_version: 1,
        enabled: false,
        selection: 'new_tabs',
        input: 'responses_v2',
        snapshot_schema: 1,
        event_schema: 1,
        control_schema: 1,
        live_fragments: true,
        snapshot: '/api/v2/agents/{agent_id}/events/snapshot',
        events: '/api/v2/agents/{agent_id}/events',
        controls: '/api/v2/agents/{agent_id}/responses/{response_id}/controls',
        command_status: '/api/v2/commands/{command_id}',
      }),
    );
    fixture.client.readAgentTranscript = async () => ({
      agentId: 'agt_1',
      throughSeq: 4,
      turns: [
        {
          turnSequence: 1,
          startedAt: '2026-11-03T00:00:00Z',
          ownerMessage: 'Compare PostgreSQL and SQLite.',
          agentMessages: ['The comparison is complete.'],
          outcome: { type: 'completed' },
          messages: [
            {
              sequence: 2,
              role: 'user',
              content: 'Compare PostgreSQL and SQLite.',
              appendedAt: '2026-11-03T00:00:00Z',
            },
            {
              sequence: 3,
              role: 'assistant',
              content: 'The comparison is complete.',
              appendedAt: '2026-11-03T00:00:00Z',
            },
          ],
        },
      ],
    });
    fixture.client.readAgentEvents = async () => ({ items: [] });
    fixture.client.replayAgent = async (_agent, options) =>
      new Promise<void>((resolve) => {
        options.signal?.addEventListener('abort', () => resolve(), { once: true });
      });
    fixture.client.runAgent = (request, options) => fixture.sdk.agui.run(request, options);
    const fetch = fixture.fetch.getMockImplementation()!;
    fixture.fetch.mockImplementation((input, init) =>
      String(input) === '/api/v1/agui/run' ? fixture.post(init) : fetch(input, init),
    );
  }
  fixture.snapshot.mockImplementation(async () =>
    json(
      page(
        [
          item(1, { type: 'turn_started', turn_sequence: 1 }),
          item(2, { type: 'message', role: 'user', content: 'Compare PostgreSQL and SQLite.' }),
          item(3, {
            type: 'message',
            role: 'assistant',
            content: 'PostgreSQL supports concurrent writers; SQLite suits local storage.',
          }),
          item(4, { type: 'turn_completed', turn_sequence: 1 }),
        ],
        4,
      ),
    ),
  );
  function Conversation({ viewer = 'alice' }: { viewer?: string }) {
    const controller = useAgentController(fixture.client, 'agt_1', { viewerId: viewer });
    return (
      <CmaClientProvider client={fixture.client}>
        <AgentSurface
          agentId="agt_1"
          viewerId={viewer}
          lifecycleClient={fixture.client}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
          onNavigate={() => {}}
        />
      </CmaClientProvider>
    );
  }
  const view = render(<Conversation />);
  const field = await screen.findByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep the budget limit.' } });
  const suggestion = await screen.findByRole('button', {
    name: 'Add explicit comparison criteria and explain the trade-offs.',
  });
  return {
    fixture,
    field,
    suggestion,
    changeViewer: (viewer: string) => view.rerender(<Conversation viewer={viewer} />),
    remount: () => {
      view.unmount();
      return render(<Conversation />);
    },
  };
}

it('recovers a double-clicked suggestion with its original request while retaining the manual draft', async () => {
  const { fixture, field, suggestion } = await setup();
  fixture.post.mockRejectedValueOnce(new TypeError('Response lost'));
  fireEvent.click(suggestion);
  fireEvent.click(suggestion);
  await screen.findByText('Awaiting confirmation');
  expect(field).toHaveValue('Keep the budget limit.');
  expect(suggestion).toBeDisabled();
  const original = fixture.post.mock.calls[0]![0]!;
  expect(JSON.parse(String(original.body))).toEqual({
    input: 'Add explicit comparison criteria and explain the trade-offs.',
    stream: true,
    answer_context: { turn_sequence: 1, answer_sequence: 3 },
  });
  fixture.post.mockResolvedValueOnce(accepted());
  fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument(),
  );
  expect(fixture.post.mock.calls[1]![0]!.body).toBe(original.body);
  expect(new Headers(fixture.post.mock.calls[1]![0]!.headers).get('Idempotency-Key')).toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  await act(async () => {
    fixture.durable(5, { type: 'turn_started', turn_sequence: 5 });
    fixture.durable(
      6,
      {
        type: 'message',
        role: 'user',
        content: 'Add explicit comparison criteria and explain the trade-offs.',
      },
      null,
      'cmd_1',
    );
  });
  const messages = screen
    .getAllByRole('article', { name: 'You' })
    .filter((message) =>
      within(message).queryByText('Add explicit comparison criteria and explain the trade-offs.'),
    );
  expect(messages).toHaveLength(1);
  expect(field).toHaveValue('Keep the budget limit.');
});

it('allows the same visible suggestion to be retried after a definitive refusal', async () => {
  const { fixture, field, suggestion } = await setup();
  fixture.post.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'invalid_response_input',
          message: 'Input refused',
          param: 'input',
        },
      },
      422,
    ),
  );
  fireEvent.click(suggestion);
  await screen.findByText('Not sent');
  await waitFor(() => expect(suggestion).toBeEnabled());
  fireEvent.click(suggestion);
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument(),
  );
  expect(field).toHaveValue('Keep the budget limit.');
  expect(JSON.parse(String(fixture.post.mock.calls[1]![0]!.body)).input).toBe(
    'Add explicit comparison criteria and explain the trade-offs.',
  );
});

it('removes a suggestion when another tab has invalidated its answer at admission', async () => {
  const { fixture, field, suggestion } = await setup();
  fixture.post.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'agent_command_not_admissible',
          message: 'This answer has changed.',
          param: 'answer_context',
        },
      },
      409,
    ),
  );
  fireEvent.click(suggestion);
  await screen.findByText('Not sent');
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  expect(field).toHaveValue('Keep the budget limit.');
});

it('does not restore the previous answer suggestions when a newer empty turn fails', async () => {
  const { fixture, field } = await setup();
  await act(async () => {
    fixture.durable(5, { type: 'turn_started', turn_sequence: 5 });
    fixture.durable(6, {
      type: 'turn_failed',
      turn_sequence: 5,
      failure: { class: 'runtime', detail: 'Unavailable' },
    });
  });
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  expect(field).toHaveValue('Keep the budget limit.');
});

it('recovers an AG-UI suggestion using the same key, scope and visible text', async () => {
  const { fixture, field, suggestion } = await setup(true);
  fixture.post.mockRejectedValueOnce(new TypeError('Receipt lost'));
  fireEvent.click(suggestion);
  await screen.findByText('Awaiting confirmation');
  const recovery = await screen.findByRole('button', { name: 'Recover pending request' });
  const original = fixture.post.mock.calls[0]![0]!;
  fixture.post.mockResolvedValueOnce(
    new Response(
      [
        { type: 'RUN_STARTED', threadId: 'agt_1', runId: '5' },
        { type: 'RUN_FINISHED', threadId: 'agt_1', runId: '5' },
      ]
        .map((event) => `data: ${JSON.stringify(event)}\n\n`)
        .join(''),
      { headers: { 'content-type': 'text/event-stream' } },
    ),
  );
  fireEvent.click(recovery);
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(2));
  expect(fixture.post.mock.calls[1]![0]!.body).toBe(original.body);
  expect(new Headers(fixture.post.mock.calls[1]![0]!.headers).get('Idempotency-Key')).toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  expect(JSON.parse(String(original.body)).forwardedProps).toEqual({
    answerContext: { turn_sequence: 1, answer_sequence: 3 },
  });
  expect(field).toHaveValue('Keep the budget limit.');
});

it('keeps a lost suggestion receipt locked after refreshing the conversation', async () => {
  const { fixture, suggestion, remount } = await setup();
  fixture.post.mockRejectedValueOnce(new TypeError('Receipt lost'));
  fireEvent.click(suggestion);
  await screen.findByText('Awaiting confirmation');
  remount();
  const restored = await screen.findByRole('button', {
    name: 'Add explicit comparison criteria and explain the trade-offs.',
  });
  expect(restored).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Recover pending request' })).toBeEnabled();
});

it('keeps the accepted answer suggestion collapsed after refreshing the conversation', async () => {
  const { suggestion, remount } = await setup();
  fireEvent.click(suggestion);
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument(),
  );
  remount();
  await screen.findByText('PostgreSQL supports concurrent writers; SQLite suits local storage.');
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
});

it('ignores the old suggestion when the viewer changes before its receipt', async () => {
  const { fixture, suggestion, changeViewer } = await setup();
  let release!: (response: Response) => void;
  fixture.post.mockImplementationOnce(
    () =>
      new Promise<Response>((resolve) => {
        release = resolve;
      }),
  );
  fireEvent.click(suggestion);
  await waitFor(() => expect(fixture.post).toHaveBeenCalledOnce());
  fixture.currentUser.mockResolvedValue('bob');
  changeViewer('bob');
  await act(async () => {
    release(accepted());
  });
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  expect(
    screen.queryByText('Add explicit comparison criteria and explain the trade-offs.'),
  ).not.toBeInTheDocument();
});

it('does not offer a template for a completed unrelated follow-up', async () => {
  const { fixture, field } = await setup();
  await act(async () => {
    fixture.durable(5, { type: 'turn_started', turn_sequence: 5 });
    fixture.durable(6, { type: 'message', role: 'user', content: 'Hello.' });
    fixture.durable(7, { type: 'message', role: 'assistant', content: 'Hello there.' });
    fixture.durable(8, { type: 'turn_completed', turn_sequence: 5 });
  });
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  expect(field).toBeEnabled();
  expect(field).toHaveValue('Keep the budget limit.');
});

it('disables suggestions while a manual message is awaiting confirmation', async () => {
  const { fixture, suggestion } = await setup();
  fixture.post.mockRejectedValueOnce(new TypeError('Receipt lost'));
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByText('Awaiting confirmation');
  expect(suggestion).toBeDisabled();
});
