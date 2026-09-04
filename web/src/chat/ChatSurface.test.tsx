import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { NO_CAPABILITIES, ProtocolError } from '@chronoai/cma-sdk';
import type {
  ConversationMessageItem,
  ResponsesConnectionState,
  ResponsesConversationItem,
  ResponsesConversationItemList,
  ResponsesResponse,
  ResponsesStreamEvent,
  ResponseToolActivity,
  ResponseWorkingNotes,
} from '@chronoai/cma-sdk';
import { describe, expect, it, vi } from 'vitest';
import type { ComponentProps } from 'react';
import type { Session } from '../sessions/types';
import {
  ChatSurface,
  latestResponseId,
  projectChatHistory,
  readAllChatItems,
  type ChatResponsesConnection,
} from './ChatSurface';
import { activityDuration, activitySummary } from './Thinking';
import type { ThinkingActivity } from './Thinking';

const CHAT_ID = 'chat_test';

function session(extra: Partial<Session> = {}): Session {
  return {
    id: CHAT_ID,
    kernel: 'codex',
    lifecycle: 'running',
    createdAt: Date.parse('2026-09-01T03:00:00Z'),
    capabilities: { ...NO_CAPABILITIES, conversation: true },
    chat: {
      worktreeName: 'chat-1',
      worktreePath: '/home/chrono/project/worktrees/chat-1',
      title: 'Release readiness',
    },
    ...extra,
  };
}

function message(id: string, role: 'user' | 'assistant', text: string): ConversationMessageItem {
  return {
    id,
    type: 'message',
    status: 'completed',
    role,
    content: [
      {
        type: role === 'user' ? 'input_text' : 'output_text',
        text,
        ...(role === 'user' ? {} : { annotations: [], logprobs: [] }),
      },
    ],
  };
}

function tool(extra: Partial<ResponseToolActivity> = {}): ResponseToolActivity {
  return {
    id: 'tool_chat_test_3',
    type: 'cma_chat.tool_activity',
    call_id: 'call-1',
    kind: 'shell',
    target: 'npm test',
    status: 'completed',
    duration_ms: 1450,
    arguments: { text: '{"cwd":"/work/chat-1"}', truncated: false },
    result: { text: '42 tests passed', truncated: true },
    ...extra,
  };
}

function thinkingActivity(extra: Partial<ThinkingActivity> = {}): ThinkingActivity {
  return {
    id: 'call-1',
    kind: 'shell',
    target: 'npm test',
    status: 'completed',
    durationMs: 1450,
    ...extra,
  };
}

function workingNotes(extra: Partial<ResponseWorkingNotes> = {}): ResponseWorkingNotes {
  return {
    id: 'work_chat_test_1',
    type: 'cma_chat.working_notes',
    response_id: 'resp_chat_test_1',
    reasoning: 'checking the release gates',
    messages: ['I checked the release inputs.'],
    ...extra,
  };
}

function historyItems(): readonly ResponsesConversationItem[] {
  // The public endpoint is newest first.
  return [
    message('msg_chat_test_1', 'assistant', 'Everything is ready.'),
    workingNotes(),
    tool(),
    message('msg_chat_test_2', 'user', 'Check the release.'),
  ];
}

function page(
  data: readonly ResponsesConversationItem[],
  hasMore = false,
): ResponsesConversationItemList {
  return {
    object: 'list',
    data,
    first_id: data[0]?.id ?? null,
    last_id: data.at(-1)?.id ?? null,
    has_more: hasMore,
  };
}

function response(
  status: ResponsesResponse['status'],
  output = 'Everything is ready.',
  extra: Partial<ResponsesResponse> = {},
): ResponsesResponse {
  return {
    id: 'resp_chat_test_1',
    object: 'response',
    created_at: 1_788_228_800,
    status,
    background: false,
    error: status === 'failed' ? { code: 'runtime_failed', message: 'The runtime exited.' } : null,
    incomplete_details: null,
    instructions: null,
    max_output_tokens: null,
    model: 'configured-chat-model',
    output:
      output === ''
        ? []
        : [
            {
              id: 'msg_chat_test_1',
              type: 'message',
              status: 'completed',
              role: 'assistant',
              content: [{ type: 'output_text', text: output, annotations: [], logprobs: [] }],
            },
          ],
    parallel_tool_calls: false,
    previous_response_id: null,
    reasoning: { effort: null, summary: null },
    store: true,
    temperature: null,
    text: { format: { type: 'text' } },
    tool_choice: 'none',
    tools: [],
    top_p: null,
    truncation: 'disabled',
    usage: null,
    metadata: {},
    ...extra,
  };
}

function TestChatSurface(props: ComponentProps<typeof ChatSurface>) {
  return (
    <ChatSurface
      readResponse={(id) => Promise.resolve(response('completed', undefined, { id }))}
      {...props}
    />
  );
}

class ControlledConnection implements ChatResponsesConnection {
  #state: ResponsesConnectionState = {
    outputText: '',
    reasoningText: '',
    toolActivities: [],
  };
  readonly #events: (ResponsesStreamEvent | null)[] = [];
  readonly #waiting: ((event: ResponsesStreamEvent | null) => void)[] = [];
  cancelled = false;

  get state(): ResponsesConnectionState {
    return this.#state;
  }

  advance(event: ResponsesStreamEvent, state: ResponsesConnectionState): void {
    this.#state = state;
    const waiting = this.#waiting.shift();
    if (waiting === undefined) this.#events.push(event);
    else waiting(event);
  }

  cancel(): void {
    this.cancelled = true;
    const waiting = this.#waiting.shift();
    if (waiting === undefined) this.#events.push(null);
    else waiting(null);
  }

  async *[Symbol.asyncIterator](): AsyncIterator<ResponsesStreamEvent> {
    while (true) {
      const queued = this.#events.shift();
      const event =
        queued === undefined
          ? await new Promise<ResponsesStreamEvent | null>((resolve) => {
              this.#waiting.push(resolve);
            })
          : queued;
      if (event === null) return;
      yield event;
      if (['response.completed', 'response.failed', 'response.incomplete'].includes(event.type)) {
        return;
      }
    }
  }
}

class InterruptedConnection implements ChatResponsesConnection {
  cancelled = false;

  constructor(readonly state: ResponsesConnectionState) {}

  cancel(): void {
    this.cancelled = true;
  }

  async *[Symbol.asyncIterator](): AsyncIterator<ResponsesStreamEvent> {
    yield await Promise.reject<ResponsesStreamEvent>(new Error('socket reset'));
  }
}

function createdEvent(current: ResponsesResponse): ResponsesStreamEvent {
  return { type: 'response.created', sequence_number: 0, response: current };
}

function inProgressEvent(current: ResponsesResponse): ResponsesStreamEvent {
  return { type: 'response.in_progress', sequence_number: 1, response: current };
}

function reasoningEvent(delta: string): ResponsesStreamEvent {
  return {
    type: 'response.reasoning_summary_text.delta',
    sequence_number: 1,
    item_id: 'rs_chat_test_1',
    output_index: 0,
    summary_index: 0,
    delta,
  };
}

function toolEvent(item: ResponseToolActivity): ResponsesStreamEvent {
  return {
    type: 'response.cma_chat.tool_activity',
    sequence_number: 2,
    response_id: 'resp_chat_test_1',
    item,
  };
}

function outputEvent(delta: string): ResponsesStreamEvent {
  return {
    type: 'response.output_text.delta',
    sequence_number: 3,
    item_id: 'msg_chat_test_1',
    output_index: 0,
    content_index: 0,
    delta,
    logprobs: [],
  };
}

describe('Responses history projection', () => {
  it('keeps messages in chronology and folds every tool in one Thinking entry per turn', () => {
    const secondTool = tool({
      id: 'tool_chat_test_4',
      call_id: 'call-2',
      kind: 'read',
      target: 'package.json',
      duration_ms: 20,
    });
    const projected = projectChatHistory([
      message('msg_chat_test_8', 'assistant', 'Second answer'),
      message('msg_chat_test_9', 'user', 'Second question'),
      message('msg_chat_test_1', 'assistant', 'First answer'),
      secondTool,
      tool(),
      message('msg_chat_test_2', 'user', 'First question'),
    ]);

    expect(projected.map((entry) => entry.kind)).toEqual([
      'message',
      'thinking',
      'message',
      'message',
      'message',
    ]);
    const thinking = projected[1];
    expect(thinking?.kind).toBe('thinking');
    if (thinking?.kind === 'thinking') {
      expect(thinking.activities.map((activity) => activity.target)).toEqual([
        'npm test',
        'package.json',
      ]);
    }
  });

  it('folds repeated tool updates by call id without moving their original order', () => {
    const projected = projectChatHistory([
      message('msg_chat_test_1', 'assistant', 'Done'),
      tool({ id: 'tool-update', status: 'completed', result: { text: 'done', truncated: false } }),
      tool({ id: 'tool-start', status: 'started', duration_ms: null, result: null }),
      message('msg_chat_test_2', 'user', 'Run it'),
    ]);
    const thinking = projected.find((entry) => entry.kind === 'thinking');
    expect(thinking?.kind).toBe('thinking');
    if (thinking?.kind === 'thinking') {
      expect(thinking.activities).toHaveLength(1);
      expect(thinking.activities[0]).toMatchObject({
        status: 'completed',
        result: { text: 'done' },
      });
    }
  });

  it('derives the latest linear fence from completed and output-less terminal turns', () => {
    expect(latestResponseId(CHAT_ID, historyItems())).toBe('resp_chat_test_1');
    expect(latestResponseId(CHAT_ID, [message('msg_chat_test_9', 'user', 'failed turn')])).toBe(
      'resp_chat_test_8',
    );
    expect(latestResponseId(CHAT_ID, [])).toBeNull();
    expect(latestResponseId(CHAT_ID, [message('not-a-projected-id', 'assistant', 'bad')])).toBe(
      undefined,
    );
  });

  it('walks newest-first pages without reversing or dropping their boundary item', async () => {
    const first = [message('msg_chat_test_8', 'assistant', 'new'), tool({ id: 'cursor' })];
    const second = [message('msg_chat_test_9', 'user', 'old')];
    const list = vi
      .fn()
      .mockResolvedValueOnce(page(first, true))
      .mockResolvedValueOnce(page(second));

    await expect(readAllChatItems(CHAT_ID, list)).resolves.toEqual([...first, ...second]);
    expect(list).toHaveBeenNthCalledWith(1, CHAT_ID, { limit: 100 }, undefined);
    expect(list).toHaveBeenNthCalledWith(2, CHAT_ID, { after: 'cursor', limit: 100 }, undefined);
  });

  it('fails closed on a repeated item or a page that cannot advance', async () => {
    const repeated = message('msg_chat_test_2', 'user', 'same');
    const duplicate = vi
      .fn()
      .mockResolvedValueOnce(page([repeated], true))
      .mockResolvedValueOnce(page([repeated]));
    await expect(readAllChatItems(CHAT_ID, duplicate)).rejects.toBeInstanceOf(ProtocolError);

    const stuck = vi.fn().mockResolvedValue(page([], true));
    await expect(readAllChatItems(CHAT_ID, stuck)).rejects.toBeInstanceOf(ProtocolError);
  });
});

describe('the reduced Chat surface', () => {
  it('reconstructs retained working notes and the separate final answer on reload', async () => {
    const listItems = vi.fn().mockResolvedValue(page(historyItems()));
    const { container } = render(<TestChatSurface session={session()} listItems={listItems} />);

    expect(await screen.findByText('Everything is ready.')).toBeInTheDocument();
    expect(screen.getByText('Check the release.')).toBeInTheDocument();
    const assistant = screen.getByText('Everything is ready.').closest('.turn');
    expect(assistant).toHaveAttribute('data-role', 'assistant');
    expect(container.querySelectorAll('.thinking')).toHaveLength(1);
    expect(container.querySelector('.turn--tool')).toBeNull();
    expect(screen.queryByText(/private chain of thought/i)).toBeNull();

    const fold = screen.getByRole('button', { name: /retained in this conversation/i });
    expect(fold).toHaveAttribute('aria-expanded', 'true');
    const body = container.querySelector('.thinking__body');
    expect(body).not.toBeNull();
    expect(body).toHaveTextContent('checking the release gates');
    expect(body).toHaveTextContent('I checked the release inputs.');
    expect(within(body as HTMLElement).getByText('shell - npm test - completed - 1.4s')).toBe(
      within(body as HTMLElement).getByText('shell - npm test - completed - 1.4s'),
    );
    const activityBodies = body?.querySelectorAll('.thinking__activitybody');
    expect(activityBodies).toHaveLength(2);
    for (const activityBody of activityBodies ?? []) {
      expect(activityBody).toHaveAttribute('open');
    }
    expect(body).toHaveTextContent('{"cwd":"/work/chat-1"}');
    expect(body).toHaveTextContent('42 tests passed');
    expect(body).toHaveTextContent('The recorded body was truncated.');
  });

  it('asserts every hidden row of the Chat reduction and keeps only the empty-state title', async () => {
    const user = userEvent.setup();
    const { container } = render(
      <TestChatSurface session={session()} listItems={vi.fn().mockResolvedValue(page([]))} />,
    );
    expect(await screen.findByText('Ready when you are.')).toBeInTheDocument();
    expect(screen.queryByText(/press/i)).toBeNull();
    expect(container.querySelector('.stream__openinghint')).toBeNull();
    expect(container.querySelector('.usage')).toBeNull();
    expect(container.querySelector('.context')).toBeNull();
    expect(container.querySelector('.resources')).toBeNull();
    expect(container.querySelector('.resources__pressure')).toBeNull();
    expect(container.querySelector('.stage')).toBeNull();
    expect(container.querySelector('.stage__mode')).toBeNull();
    expect(container.querySelector('.turn--tool')).toBeNull();

    const field = screen.getByRole('textbox', { name: 'Message' });
    await user.type(field, '/status');
    expect(container.querySelector('.commandmenu')).toBeNull();
    expect(field).toHaveValue('/status');
  });

  it('keeps live working notes through completion and moves only the final answer outside', async () => {
    const user = userEvent.setup();
    const connection = new ControlledConnection();
    let currentItems: readonly ResponsesConversationItem[] = [];
    const listItems = vi.fn().mockImplementation(() => Promise.resolve(page(currentItems)));
    const createTurn = vi.fn().mockResolvedValue(connection);
    const { container } = render(
      <TestChatSurface
        session={session()}
        listItems={listItems}
        createTurn={createTurn}
        readResponse={vi.fn()}
      />,
    );
    const field = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(field, 'Check the release.');
    await user.click(screen.getByRole('button', { name: 'Send message' }));

    await waitFor(() => expect(createTurn).toHaveBeenCalledOnce());
    expect(createTurn).toHaveBeenCalledWith(
      { conversation: CHAT_ID, input: 'Check the release.' },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(screen.getByText('Check the release.')).toBeInTheDocument();

    const inProgress = response('in_progress', '');
    await act(async () => {
      connection.advance(createdEvent(inProgress), {
        outputText: '',
        reasoningText: '',
        toolActivities: [],
        response: inProgress,
      });
    });
    await act(async () => {
      connection.advance(reasoningEvent('checking the release gates'), {
        outputText: '',
        reasoningText: 'checking the release gates',
        toolActivities: [],
        response: inProgress,
      });
    });
    const liveNotes = screen.getByRole('button', { name: /retained in this conversation/i });
    expect(liveNotes).toHaveAttribute('aria-expanded', 'true');
    expect(container.querySelector('.thinking__body')).toHaveTextContent(
      'checking the release gates',
    );
    expect(screen.queryByText('Working')).toBeNull();

    const runningTool = tool({ status: 'started', duration_ms: null, result: null });
    await act(async () => {
      connection.advance(toolEvent(runningTool), {
        outputText: '',
        reasoningText: 'checking the release gates',
        toolActivities: [runningTool],
        response: inProgress,
      });
    });
    expect(screen.getByText('shell - npm test - started - ongoing')).toBeInTheDocument();

    await act(async () => {
      connection.advance(outputEvent('Everything is ready.'), {
        outputText: 'Everything is ready.',
        reasoningText: 'checking the release gates',
        toolActivities: [tool()],
        response: inProgress,
      });
    });
    expect(screen.getByText('Everything is ready.').closest('.thinking')).not.toBeNull();
    expect(screen.getByText('Everything is ready.').closest('.turn')).toBeNull();

    currentItems = historyItems();
    const complete = response('completed');
    await act(async () => {
      connection.advance(
        { type: 'response.completed', sequence_number: 4, response: complete },
        {
          outputText: 'Everything is ready.',
          reasoningText: 'checking the release gates',
          toolActivities: [tool()],
          response: complete,
          terminal: complete,
        },
      );
    });

    await waitFor(() => expect(listItems).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.getByText('checking the release gates')).toBeInTheDocument());
    expect(screen.getByText('Everything is ready.').closest('.turn')).toHaveAttribute(
      'data-role',
      'assistant',
    );
    expect(container.querySelectorAll('.thinking')).toHaveLength(1);
    expect(container.querySelector('.thinking__body')).toHaveTextContent(
      'shell - npm test - completed - 1.4s',
    );
  });

  it('uses the latest durable response as the next turn fence', async () => {
    const user = userEvent.setup();
    const connection = new ControlledConnection();
    const createTurn = vi.fn().mockResolvedValue(connection);
    render(
      <TestChatSurface
        session={session()}
        listItems={vi.fn().mockResolvedValue(page(historyItems()))}
        createTurn={createTurn}
      />,
    );
    const field = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(field, 'One more check');
    await user.click(screen.getByRole('button', { name: 'Send message' }));
    expect(createTurn).toHaveBeenCalledWith(
      {
        conversation: CHAT_ID,
        input: 'One more check',
        previous_response_id: 'resp_chat_test_1',
      },
      expect.anything(),
    );
  });

  it('recovers an in-flight response after switching away and back without cancelling it', async () => {
    const user = userEvent.setup();
    const connection = new ControlledConnection();
    const createTurn = vi.fn().mockResolvedValue(connection);
    const chatOne = session({ id: 'chat_one' });
    const chatTwo = session({
      id: 'chat_two',
      chat: {
        worktreeName: 'chat-2',
        worktreePath: '/home/chrono/project/worktrees/chat-2',
        title: 'Second chat',
      },
    });
    let chatOneItems: readonly ResponsesConversationItem[] = [];
    const listItems = vi.fn((id: string) =>
      Promise.resolve(page(id === 'chat_one' ? chatOneItems : [])),
    );
    const intermediateText = 'Still checking the active turn.';
    const running = response('in_progress', intermediateText, {
      id: 'resp_chat_one_1',
      previous_response_id: null,
    });
    const completed = response('completed', 'Recovered answer', {
      id: 'resp_chat_one_1',
      previous_response_id: null,
      output: [
        {
          id: 'msg_chat_one_1',
          type: 'message',
          status: 'completed',
          role: 'assistant',
          content: [
            { type: 'output_text', text: 'Recovered answer', annotations: [], logprobs: [] },
          ],
        },
      ],
    });
    const activeChatOneItems = (): readonly ResponsesConversationItem[] => [
      workingNotes({
        id: 'work_chat_one_1',
        response_id: 'resp_chat_one_1',
        reasoning: 'checking after navigation',
        messages: [intermediateText],
      }),
      message('msg_chat_one_2', 'user', 'Keep working'),
    ];
    let finishRecovery!: () => void;
    const recoveryGate = new Promise<void>((resolve) => {
      finishRecovery = resolve;
    });
    const readResponse = vi
      .fn()
      .mockImplementationOnce(() => {
        chatOneItems = activeChatOneItems();
        return Promise.resolve(running);
      })
      .mockImplementationOnce(async () => {
        await recoveryGate;
        chatOneItems = [
          message('msg_chat_one_1', 'assistant', 'Recovered answer'),
          ...activeChatOneItems(),
        ];
        return completed;
      });
    const view = render(
      <TestChatSurface
        session={chatOne}
        listItems={listItems}
        createTurn={createTurn}
        readResponse={readResponse}
      />,
    );

    const field = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(field, 'Keep working');
    await user.click(screen.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(createTurn).toHaveBeenCalledOnce());
    await act(async () => {
      connection.advance(createdEvent(running), {
        outputText: '',
        reasoningText: '',
        toolActivities: [],
        response: running,
      });
    });
    chatOneItems = [message('msg_chat_one_2', 'user', 'Keep working')];

    view.rerender(
      <TestChatSurface
        session={chatTwo}
        listItems={listItems}
        createTurn={createTurn}
        readResponse={readResponse}
      />,
    );
    await screen.findByText('Ready when you are.');
    expect(connection.cancelled).toBe(false);

    view.rerender(
      <TestChatSurface
        session={chatOne}
        listItems={listItems}
        createTurn={createTurn}
        readResponse={readResponse}
      />,
    );
    await waitFor(() =>
      expect(readResponse).toHaveBeenCalledWith('resp_chat_one_1', expect.any(AbortSignal)),
    );
    expect(screen.getByRole('status')).toHaveTextContent("Checking this chat's latest response.");
    expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull();
    const recoveredIntermediate = await screen.findByText(intermediateText);
    expect(screen.getAllByText(intermediateText)).toHaveLength(1);
    expect(recoveredIntermediate.closest('.thinking')).not.toBeNull();
    expect(recoveredIntermediate.closest('.turn')).toBeNull();
    expect(view.container.querySelectorAll('.thinking')).toHaveLength(1);
    expect(view.container.querySelector('.stream__waiting')).toBeNull();

    await waitFor(() => expect(readResponse).toHaveBeenCalledTimes(2), { timeout: 2_500 });
    await act(async () => {
      finishRecovery();
      await recoveryGate;
    });
    expect(await screen.findByText('Recovered answer', {}, { timeout: 2_500 })).toBeInTheDocument();
    expect(screen.getByText('Recovered answer').closest('.turn')).toHaveAttribute(
      'data-role',
      'assistant',
    );
    expect(screen.getAllByText(intermediateText)).toHaveLength(1);
    expect(screen.getByText(intermediateText).closest('.thinking')).not.toBeNull();
    expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
    expect(screen.getAllByText('Keep working')).toHaveLength(1);
    connection.cancel();
  });

  it('retrieves and durably reloads a terminal response after the stream throws', async () => {
    const user = userEvent.setup();
    const inProgress = response('in_progress', '');
    const connection = new InterruptedConnection({
      outputText: '',
      reasoningText: 'transient reasoning',
      toolActivities: [],
      response: inProgress,
    });
    const recovered = response('completed', 'Recovered from the response resource.');
    const listItems = vi
      .fn()
      .mockResolvedValueOnce(page([]))
      .mockResolvedValueOnce(
        page([
          message('msg_chat_test_1', 'assistant', 'Recovered from the response resource.'),
          message('msg_chat_test_2', 'user', 'Recover this turn'),
        ]),
      );
    const readResponse = vi.fn().mockResolvedValue(recovered);
    render(
      <TestChatSurface
        session={session()}
        listItems={listItems}
        createTurn={vi.fn().mockResolvedValue(connection)}
        readResponse={readResponse}
      />,
    );

    const field = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(field, 'Recover this turn');
    await user.click(screen.getByRole('button', { name: 'Send message' }));

    expect(await screen.findByText('Recovered from the response resource.')).toBeInTheDocument();
    expect(readResponse).toHaveBeenCalledWith('resp_chat_test_1', expect.any(AbortSignal));
    expect(listItems).toHaveBeenCalledTimes(2);
    expect(screen.getAllByText('Recover this turn')).toHaveLength(1);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByText('transient reasoning')).toBeNull();
  });

  it('keeps a refused draft and reports history and admission failures explicitly', async () => {
    const user = userEvent.setup();
    const failedHistory = render(
      <TestChatSurface
        session={session()}
        listItems={vi.fn().mockRejectedValue(new Error('offline'))}
      />,
    );
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /history could not be read.*offline/i,
    );
    expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull();
    failedHistory.unmount();

    const createTurn = vi.fn().mockRejectedValue(new Error('admission refused'));
    render(
      <TestChatSurface
        session={session()}
        listItems={vi.fn().mockResolvedValue(page([]))}
        createTurn={createTurn}
      />,
    );
    const field = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(field, 'Do not lose this draft');
    await user.click(screen.getByRole('button', { name: 'Send message' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(
      /message was not sent.*admission refused/i,
    );
    expect(field).toHaveValue('Do not lose this draft');
  });

  it('renders a terminal response failure in the turn and cancels a live reader on unmount', async () => {
    const user = userEvent.setup();
    const connection = new ControlledConnection();
    let currentItems: readonly ResponsesConversationItem[] = [];
    const listItems = vi.fn().mockImplementation(() => Promise.resolve(page(currentItems)));
    const view = render(
      <TestChatSurface
        session={session()}
        listItems={listItems}
        createTurn={vi.fn().mockResolvedValue(connection)}
      />,
    );
    const field = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(field, 'Run the check');
    await user.click(screen.getByRole('button', { name: 'Send message' }));
    currentItems = [message('msg_chat_test_2', 'user', 'Run the check')];
    const failed = response('failed', '');
    await act(async () => {
      connection.advance(
        { type: 'response.failed', sequence_number: 0, response: failed },
        {
          outputText: '',
          reasoningText: '',
          toolActivities: [],
          response: failed,
          terminal: failed,
        },
      );
    });
    await waitFor(() => expect(screen.getByText('The runtime exited.')).toBeInTheDocument());
    expect(screen.getByText('The runtime exited.').closest('.failure')).not.toBeNull();
    expect(screen.getAllByText('Run the check')).toHaveLength(1);
    view.unmount();
    expect(connection.cancelled).toBe(false);

    const live = new ControlledConnection();
    const liveView = render(
      <TestChatSurface
        session={session()}
        listItems={vi.fn().mockResolvedValue(page([]))}
        createTurn={vi.fn().mockResolvedValue(live)}
      />,
    );
    fireEvent.change(await screen.findByRole('textbox', { name: 'Message' }), {
      target: { value: 'keep running' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(screen.getByText('keep running')).toBeInTheDocument());
    liveView.unmount();
    expect(live.cancelled).toBe(false);
  });

  it('keeps paused history visible and accepts a revive only when its created stream progresses', async () => {
    const user = userEvent.setup();
    const notice = 'Your current chat session has been idle for 47min and has been auto-paused.';
    const paused = session({
      lifecycle: 'idle',
      stopReason: 'idle_timeout',
      chat: {
        worktreeName: 'chat-1',
        worktreePath: '/home/chrono/project/worktrees/chat-1',
        title: 'Release readiness',
        pausedNotice: notice,
      },
    });
    const connection = new ControlledConnection();
    let currentItems = historyItems();
    const listItems = vi.fn().mockImplementation(() => Promise.resolve(page(currentItems)));
    const createTurn = vi.fn().mockResolvedValue(connection);
    const view = render(
      <TestChatSurface session={paused} listItems={listItems} createTurn={createTurn} />,
    );

    expect(await screen.findByText('Everything is ready.')).toBeInTheDocument();
    expect(screen.getByText('Check the release.')).toBeInTheDocument();
    const pauseStatus = screen.getByRole('status', { name: '' });
    expect(pauseStatus).toHaveTextContent(notice);
    expect(pauseStatus.textContent).toBe(notice);
    const field = screen.getByRole('textbox', { name: 'Message' });
    expect(field).toHaveAttribute(
      'placeholder',
      'Send a message to continue with your previous chat session.',
    );
    expect(field).toBeEnabled();

    await user.type(field, 'Continue the release check');
    await user.click(screen.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(createTurn).toHaveBeenCalledOnce());
    expect(createTurn).toHaveBeenCalledWith(
      {
        conversation: CHAT_ID,
        input: 'Continue the release check',
        previous_response_id: 'resp_chat_test_1',
      },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(field).toHaveValue('Continue the release check');
    expect(
      within(screen.getByRole('log', { name: 'Conversation' })).queryByText(
        'Continue the release check',
      ),
    ).toBeNull();

    const inProgress = response('in_progress', '', {
      id: 'resp_chat_test_4',
      previous_response_id: 'resp_chat_test_1',
    });
    await act(async () => {
      connection.advance(createdEvent(inProgress), {
        outputText: '',
        reasoningText: '',
        toolActivities: [],
        response: inProgress,
      });
    });
    expect(screen.getByText('Starting…')).toHaveClass('startup__stage');
    expect(screen.queryByText('Working')).toBeNull();
    expect(field).toHaveValue('Continue the release check');
    expect(field).toBeEnabled();
    expect(screen.getByText('Everything is ready.')).toBeInTheDocument();

    await act(async () => {
      connection.advance(inProgressEvent(inProgress), {
        outputText: '',
        reasoningText: '',
        toolActivities: [],
        response: inProgress,
      });
    });
    await waitFor(() => expect(field).toHaveValue(''));
    expect(screen.queryByText('Starting…')).toBeNull();
    expect(screen.getByText('Continue the release check')).toBeInTheDocument();

    await act(async () => {
      connection.advance(outputEvent('Restored answer'), {
        outputText: 'Restored answer',
        reasoningText: '',
        toolActivities: [],
        response: inProgress,
      });
    });
    expect(screen.getByText('Restored answer')).toBeInTheDocument();

    currentItems = [
      message('msg_chat_test_4', 'assistant', 'Restored answer'),
      message('msg_chat_test_5', 'user', 'Continue the release check'),
      ...historyItems(),
    ];
    const complete = response('completed', 'Restored answer', {
      id: 'resp_chat_test_4',
      previous_response_id: 'resp_chat_test_1',
    });
    await act(async () => {
      connection.advance(
        { type: 'response.completed', sequence_number: 4, response: complete },
        {
          outputText: 'Restored answer',
          reasoningText: '',
          toolActivities: [],
          response: complete,
          terminal: complete,
        },
      );
    });
    await waitFor(() => expect(listItems).toHaveBeenCalledTimes(2));
    expect(screen.getAllByText('Continue the release check')).toHaveLength(1);
    expect(screen.getAllByText('Restored answer')).toHaveLength(1);

    view.rerender(
      <TestChatSurface session={session()} listItems={listItems} createTurn={createTurn} />,
    );
    expect(screen.queryByText(notice)).toBeNull();
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveAttribute(
      'placeholder',
      'Say what you want done',
    );
  });

  it('shows a provisioning timeout with its code and preserves the failed revive draft exactly', async () => {
    const paused = session({
      lifecycle: 'idle',
      stopReason: 'idle_timeout',
      chat: {
        worktreeName: 'chat-1',
        worktreePath: '/home/chrono/project/worktrees/chat-1',
        title: 'Release readiness',
        pausedNotice: 'Your current chat session has been idle for 15min and has been auto-paused.',
      },
    });
    const timedOutConnection = new ControlledConnection();
    const retryConnection = new ControlledConnection();
    const listItems = vi.fn().mockResolvedValue(page(historyItems()));
    const createTurn = vi
      .fn()
      .mockResolvedValueOnce(timedOutConnection)
      .mockResolvedValueOnce(retryConnection);
    const view = render(
      <TestChatSurface session={paused} listItems={listItems} createTurn={createTurn} />,
    );
    const field = await screen.findByRole('textbox', { name: 'Message' });
    const draft = '  Keep this exact.\nSecond line  ';
    fireEvent.change(field, { target: { value: draft } });
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(createTurn).toHaveBeenCalledOnce());

    const timedOut = response('failed', '', {
      id: 'resp_chat_test_0',
      error: {
        code: 'provisioning_timeout',
        message: 'The conversation did not become reachable before the readiness deadline.',
      },
    });
    await act(async () => {
      timedOutConnection.advance(
        { type: 'response.failed', sequence_number: 0, response: timedOut },
        {
          outputText: '',
          reasoningText: '',
          toolActivities: [],
          response: timedOut,
          terminal: timedOut,
        },
      );
    });

    const failure = await waitFor(() => {
      const element = view.container.querySelector('.failure');
      expect(element).not.toBeNull();
      return element as HTMLElement;
    });
    expect(failure).toHaveTextContent(
      'The conversation did not become reachable before the readiness deadline.',
    );
    expect(failure).toHaveTextContent('provisioning_timeout');
    expect(field).toHaveValue(draft);
    expect(field).toBeEnabled();
    expect(
      within(screen.getByRole('log', { name: 'Conversation' })).queryByText(
        'Keep this exact. Second line',
      ),
    ).toBeNull();

    await waitFor(() => expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
    await waitFor(() => expect(createTurn).toHaveBeenCalledTimes(2));
    expect(createTurn).toHaveBeenLastCalledWith(
      {
        conversation: CHAT_ID,
        input: 'Keep this exact.\nSecond line',
        previous_response_id: 'resp_chat_test_1',
      },
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    expect(field).toHaveValue(draft);
  });

  it('does not apply paused affordances to another idle stop reason', async () => {
    const notice = 'This sentence must not be inferred from metadata alone.';
    render(
      <TestChatSurface
        session={session({
          lifecycle: 'idle',
          stopReason: 'end_turn',
          chat: {
            worktreeName: 'chat-1',
            worktreePath: '/home/chrono/project/worktrees/chat-1',
            pausedNotice: notice,
          },
        })}
        listItems={vi.fn().mockResolvedValue(page(historyItems()))}
      />,
    );

    expect(await screen.findByText('Everything is ready.')).toBeInTheDocument();
    expect(screen.queryByText(notice)).toBeNull();
    expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent(
      'This session is paused. Resume it to carry on.',
    );
  });

  it('keeps an approval in the input seat and rejects a non-chat session', async () => {
    const listItems = vi.fn().mockResolvedValue(page([]));
    const approval = render(
      <TestChatSurface
        session={session()}
        listItems={listItems}
        approval={<div role="status">Approval required</div>}
      />,
    );
    expect(await screen.findByText('Ready when you are.')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Approval required');
    expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull();
    approval.unmount();

    render(<TestChatSurface session={session({ chat: undefined })} listItems={listItems} />);
    expect(screen.getByRole('alert')).toHaveTextContent('This session is not a chat thread.');
  });
});

describe('Thinking activity vocabulary', () => {
  it('states live, missing, millisecond, and fractional-second durations without inventing one', () => {
    expect(activityDuration(thinkingActivity({ status: 'started', durationMs: null }))).toBe(
      'ongoing',
    );
    expect(activityDuration(thinkingActivity({ status: 'completed', durationMs: null }))).toBe(
      'not reported',
    );
    expect(activityDuration(thinkingActivity({ durationMs: 42 }))).toBe('42ms');
    expect(activityDuration(thinkingActivity({ durationMs: 1250 }))).toBe('1.3s');
    expect(activitySummary(thinkingActivity({ durationMs: 2000 }))).toBe(
      'shell - npm test - completed - 2s',
    );
  });
});
