import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { useAgentDraft } from './draft';
import { accepted, harness, item, json, page } from './transport/controller.fixture';
import type { SuggestedReplySet } from './suggestions';

const suggestionText = 'Add explicit comparison criteria and explain the trade-offs.';

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
          item(3, { type: 'message', role: 'assistant', content: 'The comparison is complete.' }),
          item(4, { type: 'turn_completed', turn_sequence: 1 }),
        ],
        4,
      ),
    ),
  );
  Object.assign(fixture.client, {
    uploadAgentAttachment: async (agentId: string, file: File) => ({
      id: 'budget',
      agentId,
      name: file.name,
      path: '/home/attachments/budget.txt',
      size: file.size,
      mediaType: file.type,
      sha256: 'a'.repeat(64),
    }),
  });
  function Conversation({
    viewer = 'alice',
    agentId = 'agt_1',
    provided,
    accessUncertain = false,
  }: {
    viewer?: string;
    agentId?: string;
    provided?: SuggestedReplySet | null;
    accessUncertain?: boolean;
  }) {
    const controller = useAgentController(fixture.client, agentId, { viewerId: viewer });
    const draft = useAgentDraft(fixture.client, viewer, agentId);
    return (
      <CmaClientProvider client={fixture.client}>
        <AgentSurface
          draft={draft}
          suggestedReplies={provided}
          agentId={agentId}
          viewerId={viewer}
          accessUncertain={accessUncertain}
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
  await screen.findByRole('button', { name: suggestionText });
  const user = userEvent.setup();
  return {
    fixture,
    field,
    user,
    async attach() {
      const picker = view.container.querySelector<HTMLInputElement>('input[type="file"]')!;
      await user.upload(picker, new File(['budget <= 100'], 'budget.txt', { type: 'text/plain' }));
      await waitFor(() =>
        expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('Ready'),
      );
    },
    rerender: (props: Parameters<typeof Conversation>[0]) =>
      view.rerender(<Conversation {...props} />),
  };
}

it('cancels editing without changing the existing budget draft or attachment and restores composer focus', async () => {
  const { field, user, attach, fixture } = await setup();
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  const dialog = screen.getByRole('dialog', { name: 'Keep your draft?' });
  await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(field).toHaveFocus());
  expect(field).toHaveValue('保留预算上限');
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  expect(fixture.post).not.toHaveBeenCalled();
  expect(screen.getByRole('region', { name: 'Next you can ask' })).toBeVisible();
});

it('inserts the complete supplied suggestion into an empty composer without sending or removing attachments', async () => {
  const { field, user, attach, fixture, rerender } = await setup();
  const fullText = '  Compare latency\nand the stated costs.  ';
  rerender({
    provided: {
      id: 'context-set',
      viewerId: 'alice',
      agentId: 'agt_1',
      answerId: 'event:3',
      scope: { turn_sequence: 1, answer_sequence: 3 },
      source: { kind: 'generated', version: 'fixture-v1' },
      items: [{ id: 'latency', text: fullText, rank: 0 }],
    },
  });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(field).toHaveValue(fullText);
  expect(field).toHaveFocus();
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  expect(fixture.post).not.toHaveBeenCalled();
});

it('explicitly appends after the unchanged multiline draft and keeps its attachment', async () => {
  const { field, user, attach, fixture } = await setup();
  fireEvent.change(field, { target: { value: '保留预算上限\n不超过 100。  ' } });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  await user.click(screen.getByRole('button', { name: 'Append to draft' }));
  await waitFor(() => expect(field).toHaveFocus());
  expect(field).toHaveValue(
    '保留预算上限\n不超过 100。  \nAdd explicit comparison criteria and explain the trade-offs.',
  );
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  expect(fixture.post).not.toHaveBeenCalled();
});

it('replaces only text after confirmation and can undo the replacement with the attachment retained', async () => {
  const { field, user, attach, fixture } = await setup();
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  await user.click(screen.getByRole('button', { name: 'Replace text' }));
  await waitFor(() => expect(field).toHaveFocus());
  expect(field).toHaveValue(suggestionText);
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  await user.click(screen.getByRole('button', { name: 'Undo replacement' }));
  expect(field).toHaveValue('保留预算上限');
  expect(field).toHaveFocus();
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  expect(fixture.post).not.toHaveBeenCalled();
});

it('submits the edited body through normal admission and collapses its suggestion only after acceptance', async () => {
  const { field, user, fixture } = await setup();
  let acknowledge!: (response: Response) => void;
  fixture.post.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        acknowledge = resolve;
      }),
  );
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  fireEvent.change(field, { target: { value: '保留预算上限\nCompare latency and cost.' } });
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledOnce());
  const request = JSON.parse(String(fixture.post.mock.calls[0]![0]!.body));
  expect(request.input).toBe('保留预算上限\nCompare latency and cost.');
  expect(request.answer_context).toEqual({ turn_sequence: 1, answer_sequence: 3 });
  expect(screen.getByRole('region', { name: 'Next you can ask' })).toBeVisible();
  await act(async () => acknowledge(accepted()));
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument(),
  );
  expect(field).toHaveValue('');
  expect(screen.getByText('保留预算上限 Compare latency and cost.')).toBeVisible();
});

it('recovers an immutable uncertain request while retaining newer edited text and attachments for a fresh intent', async () => {
  const { field, user, fixture, attach } = await setup();
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  fireEvent.change(field, { target: { value: 'First edited budget request' } });
  fixture.post.mockRejectedValueOnce(new TypeError('Receipt lost'));
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  const recovery = await screen.findByRole('button', { name: 'Recover pending request' });
  const original = fixture.post.mock.calls[0]![0]!;
  expect(field).toHaveValue('First edited budget request');
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  await user.click(screen.getByRole('button', { name: 'Replace text' }));
  fireEvent.change(field, { target: { value: 'Fresh edited budget request' } });
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  await user.click(recovery);
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(2));
  expect(fixture.post.mock.calls[1]![0]!.body).toBe(original.body);
  expect(new Headers(fixture.post.mock.calls[1]![0]!.headers).get('Idempotency-Key')).toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  expect(field).toHaveValue('Fresh edited budget request');
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(3));
  const fresh = fixture.post.mock.calls[2]![0]!;
  expect(new Headers(fresh.headers).get('Idempotency-Key')).not.toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  expect(JSON.parse(String(fresh.body)).input).toContain('Fresh edited budget request');
  expect(JSON.parse(String(fresh.body)).answer_context).toBeUndefined();
});

it('does not let a late acknowledgement erase a newer replacement or its retained attachment', async () => {
  const { field, user, fixture, attach } = await setup();
  await attach();
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  let acknowledge!: (response: Response) => void;
  fixture.post.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        acknowledge = resolve;
      }),
  );
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledOnce());
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  await user.click(screen.getByRole('button', { name: 'Replace text' }));
  expect(field).toHaveValue(suggestionText);
  await act(async () => acknowledge(accepted()));
  expect(field).toHaveValue(suggestionText);
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  expect(fixture.post).toHaveBeenCalledOnce();
});

it('closes the old edit decision on account change without injecting its text or attachment into the new account', async () => {
  const { field, user, attach, rerender, fixture } = await setup();
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  expect(screen.getByRole('dialog', { name: 'Keep your draft?' })).toBeVisible();
  fixture.currentUser.mockResolvedValue('bob');
  rerender({ viewer: 'bob' });
  await waitFor(() =>
    expect(screen.queryByRole('dialog', { name: 'Keep your draft?' })).not.toBeInTheDocument(),
  );
  expect(await screen.findByRole('textbox', { name: 'Message' })).toHaveValue('');
  expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
  expect(fixture.post).not.toHaveBeenCalled();
});

it('withdraws an open edit decision when its answer expires and preserves the current draft', async () => {
  const { field, user, fixture, attach } = await setup();
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  await act(async () => {
    fixture.durable(5, { type: 'turn_started', turn_sequence: 5 });
    fixture.durable(6, {
      type: 'turn_failed',
      turn_sequence: 5,
      failure: { class: 'runtime', detail: 'Unavailable' },
    });
  });
  expect(screen.queryByRole('dialog', { name: 'Keep your draft?' })).not.toBeInTheDocument();
  expect(field).toHaveValue('保留预算上限');
  expect(field).toHaveFocus();
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  expect(fixture.post).not.toHaveBeenCalled();
});

it('keeps an expired edited source as ordinary input and rechecks the owner before sending it', async () => {
  const { field, user, fixture, rerender } = await setup();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  fireEvent.change(field, { target: { value: 'Manual budget follow-up' } });
  await act(async () => {
    fixture.durable(5, { type: 'turn_started', turn_sequence: 5 });
    fixture.durable(6, {
      type: 'turn_failed',
      turn_sequence: 5,
      failure: { class: 'runtime', detail: 'Unavailable' },
    });
  });
  expect(field).toHaveValue('Manual budget follow-up');
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  fixture.currentUser.mockResolvedValue('bob');
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByText('Your sign-in changed. Open the Agent again to continue.');
  expect(fixture.post).not.toHaveBeenCalled();
  fixture.currentUser.mockResolvedValue('alice');
  rerender({ provided: null });
  await user.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('textbox', { name: 'Message' })).toHaveValue(
    'Manual budget follow-up',
  );
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledOnce());
  expect(JSON.parse(String(fixture.post.mock.calls[0]![0]!.body))).toEqual({
    input: 'Manual budget follow-up',
    stream: true,
  });
});

it('preserves an edited draft after a stale-answer refusal and allows a deliberate ordinary send', async () => {
  const { field, user, fixture } = await setup();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  fireEvent.change(field, { target: { value: '保留预算上限\nCompare latency and cost.' } });
  fixture.post.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'agent_command_not_admissible',
          message: 'The answer changed.',
          param: 'answer_context',
        },
      },
      409,
    ),
  );
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByText('Not sent');
  expect(field).toHaveValue('保留预算上限\nCompare latency and cost.');
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(2));
  expect(JSON.parse(String(fixture.post.mock.calls[1]![0]!.body))).toEqual({
    input: '保留预算上限\nCompare latency and cost.',
    stream: true,
  });
});

it('retires replacement undo after manual editing even if the user types the same suggestion again', async () => {
  const { field, user } = await setup();
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  await user.click(screen.getByRole('button', { name: 'Replace text' }));
  expect(screen.getByRole('button', { name: 'Undo replacement' })).toBeVisible();
  fireEvent.change(field, { target: { value: 'A new manual draft' } });
  fireEvent.change(field, { target: { value: suggestionText } });
  expect(screen.queryByRole('button', { name: 'Undo replacement' })).not.toBeInTheDocument();
  expect(field).toHaveValue(suggestionText);
});

it('uses the normal AG-UI route to recover the exact edited intent while keeping newer draft text', async () => {
  const { field, user, fixture, attach } = await setup(true);
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  fireEvent.change(field, { target: { value: '保留预算上限\nCompare latency and cost.' } });
  fixture.post.mockRejectedValueOnce(new TypeError('Receipt lost'));
  await user.click(screen.getByRole('button', { name: 'Send message' }));
  const recover = await screen.findByRole('button', { name: 'Recover pending request' });
  expect(field).toHaveValue('保留预算上限\nCompare latency and cost.');
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  const original = fixture.post.mock.calls[0]![0]!;
  const body = JSON.parse(String(original.body));
  expect(body.messages[0].content).toContain('保留预算上限\nCompare latency and cost.');
  expect(body.forwardedProps.answerContext).toEqual({ turn_sequence: 1, answer_sequence: 3 });
  fireEvent.change(field, { target: { value: 'Later independent draft' } });
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
  await user.click(recover);
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(2));
  expect(fixture.post.mock.calls[1]![0]!.body).toBe(original.body);
  expect(new Headers(fixture.post.mock.calls[1]![0]!.headers).get('Idempotency-Key')).toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  expect(field).toHaveValue('Later independent draft');
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  await waitFor(() =>
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument(),
  );
});

it('keeps each Agent draft and attachment with its owner when navigation closes an edit decision', async () => {
  const { field, user, fixture, attach, rerender } = await setup(true);
  const readAgent = fixture.client.readAgent;
  fixture.client.readAgent = async (agentId, options) => {
    const detail = await readAgent(agentId, options);
    return { ...detail, id: agentId, worktree: { ...detail.worktree, agentId } };
  };
  const readTranscript = fixture.client.readAgentTranscript;
  const readUsage = fixture.client.readAgentUsage;
  fixture.client.readAgentUsage = async (agentId, options) => ({
    ...(await readUsage(agentId, options)),
    agentId,
  });
  fixture.client.readAgentTranscript = (agentId, options) =>
    agentId === 'agt_2'
      ? Promise.resolve({ agentId, throughSeq: 0, turns: [] })
      : readTranscript(agentId, options);
  fireEvent.change(field, { target: { value: '保留预算上限' } });
  await attach();
  await user.click(screen.getByRole('button', { name: 'Edit suggestion 1 before sending' }));
  rerender({ agentId: 'agt_2' });
  const otherField = await screen.findByRole('textbox', { name: 'Message' });
  expect(otherField).toHaveValue('');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
  fireEvent.change(otherField, { target: { value: 'Second Agent draft' } });
  rerender({ agentId: 'agt_1' });
  expect(await screen.findByRole('textbox', { name: 'Message' })).toHaveValue('保留预算上限');
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('budget.txt');
  rerender({ agentId: 'agt_2' });
  expect(await screen.findByRole('textbox', { name: 'Message' })).toHaveValue('Second Agent draft');
  expect(fixture.post).not.toHaveBeenCalled();
});
