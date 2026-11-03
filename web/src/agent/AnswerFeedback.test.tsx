import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { messageFeedbackClient } from '../api/rest/messageFeedback';
import type { AgentEvent, AgentTranscript } from '../api/types';
import type { MessageVote } from '../api/client';
import { agentClient } from '../test/agent';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import { useMessageFeedback } from './useMessageFeedback';

const transcript: AgentTranscript = {
  agentId: 'agent',
  throughSeq: 6,
  turns: [
    {
      turnSequence: 1,
      startedAt: '2026-11-03T01:00:00Z',
      ownerMessage: 'Review the result.',
      agentMessages: ['Checking the evidence.', 'The final answer is ready.'],
      messages: [
        {
          sequence: 2,
          role: 'user',
          content: 'Review the result.',
          appendedAt: '2026-11-03T01:00:00Z',
        },
        {
          sequence: 3,
          role: 'assistant',
          content: 'Checking the evidence.',
          appendedAt: '2026-11-03T01:00:01Z',
        },
        {
          sequence: 5,
          role: 'assistant',
          content: 'The final answer is ready.',
          appendedAt: '2026-11-03T01:00:02Z',
        },
      ],
      outcome: { type: 'completed', at: '2026-11-03T01:00:03Z' },
    },
  ],
};

function Answers({
  snapshot = transcript,
  viewer = 'owner',
  enabled = true,
  events = [],
}: {
  readonly snapshot?: AgentTranscript;
  readonly viewer?: string;
  readonly enabled?: boolean;
  readonly events?: readonly AgentEvent[];
}) {
  const feedback = useMessageFeedback(snapshot.agentId, viewer, enabled);
  return (
    <MessageStream
      conversation={events.reduce(foldAgentEvent, conversationFromTranscript(snapshot))}
      feedback={feedback}
      onApproval={() => undefined}
    />
  );
}

const client = agentClient(messageFeedbackClient());
function draw(props: Parameters<typeof Answers>[0] = {}) {
  return (
    <CmaClientProvider client={client}>
      <Answers {...props} />
    </CmaClientProvider>
  );
}

function answer(content = 'The final answer is ready.') {
  return within(screen.getByText(content).closest('article')!);
}

afterEach(() => vi.unstubAllGlobals());

it.each(['active', 'failed', 'interrupted'])(
  'withholds feedback for a %s turn while preserving copy',
  (type) => {
    vi.stubGlobal('fetch', async () => Response.json({ items: [], next: null }));
    render(
      draw({
        snapshot: {
          ...transcript,
          turns: transcript.turns.map((turn) => ({ ...turn, outcome: { type } })),
        },
      }),
    );
    expect(screen.queryByRole('button', { name: 'Helpful' })).toBeNull();
    expect(answer().getByRole('button', { name: 'Copy content' })).toBeEnabled();
  },
);

it('keeps regenerated identical answers independent and determines finality by durable sequence', async () => {
  const original = transcript.turns[0]!;
  const snapshot = {
    ...transcript,
    throughSeq: 21,
    turns: [
      {
        ...original,
        messages: [...original.messages]
          .reverse()
          .map((message) =>
            message.role === 'assistant' ? { ...message, content: 'Identical answer' } : message,
          ),
      },
      {
        ...original,
        turnSequence: 18,
        messages: [
          { ...original.messages[0]!, sequence: 19 },
          { ...original.messages[2]!, sequence: 20, content: 'Identical answer' },
        ],
      },
    ],
  };
  vi.stubGlobal('fetch', async (path: string, options?: RequestInit) => {
    if (options?.method !== 'PUT') return Response.json({ items: [], next: null });
    if (!path.endsWith('/messages/event%3A5/feedback')) throw new Error('Wrong answer identity');
    return Response.json({ message_id: 'event:5', vote: 'up' });
  });
  render(draw({ snapshot }));
  const votes = await screen.findAllByRole('button', { name: 'Helpful' });
  expect(votes).toHaveLength(2);
  await waitFor(() => expect(votes[0]).toBeEnabled());
  await userEvent.click(votes[0]!);
  await waitFor(() => expect(votes[0]).toHaveAttribute('aria-pressed', 'true'));
  expect(votes[1]).toHaveAttribute('aria-pressed', 'false');
});

it.each(['viewer', 'agent'])(
  'isolates a late write immediately when the %s changes',
  async (scope) => {
    let finish!: (response: Response) => void;
    let changed = false;
    vi.stubGlobal('fetch', async (_path: string, options?: RequestInit) => {
      if (options?.method === 'PUT')
        return new Promise<Response>((resolve) => {
          finish = resolve;
        });
      return Response.json({
        items: changed ? [] : [{ message_id: 'event:5', vote: 'up' }],
        next: null,
      });
    });
    const view = render(draw());
    await waitFor(() =>
      expect(answer().getByRole('button', { name: 'Needs improvement' })).toBeEnabled(),
    );
    await userEvent.click(answer().getByRole('button', { name: 'Needs improvement' }));
    changed = true;
    view.rerender(
      draw(
        scope === 'viewer'
          ? { viewer: 'other' }
          : { snapshot: { ...transcript, agentId: 'other' } },
      ),
    );
    expect(answer().getByRole('button', { name: 'Helpful' })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    await act(async () => finish(Response.json({ message_id: 'event:5', vote: 'down' })));
    await waitFor(() => expect(answer().getByRole('button', { name: 'Helpful' })).toBeEnabled());
    expect(answer().getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    expect(screen.queryByRole('button', { name: 'Retry vote' })).toBeNull();
  },
);

it('withholds feedback when a completed turn reports missing conversation output', async () => {
  vi.stubGlobal('fetch', async () => Response.json({ items: [], next: null }));
  const snapshot = {
    ...transcript,
    throughSeq: 5,
    turns: transcript.turns.map((turn) => ({ ...turn, outcome: { type: 'active' } })),
  };
  render(
    draw({
      snapshot,
      events: [
        {
          seq: 6,
          appendedAt: '2026-11-03T01:00:03Z',
          kind: { type: 'conversation_gap', frames: 2 },
        },
        {
          seq: 7,
          appendedAt: '2026-11-03T01:00:04Z',
          kind: { type: 'turn_completed', turn_sequence: 1 },
        },
      ],
    }),
  );
  await waitFor(() => expect(answer().getByRole('button', { name: 'Copy content' })).toBeEnabled());
  expect(screen.queryByRole('button', { name: 'Helpful' })).toBeNull();
});

it('retains an uncertain requested vote across reopening and retries it before allowing the opposite choice', async () => {
  let available = false;
  let saved: MessageVote = 'up';
  vi.stubGlobal('fetch', async (_path: string, options?: RequestInit) => {
    if (options?.method !== 'PUT')
      return Response.json({ items: [{ message_id: 'event:5', vote: saved }], next: null });
    if (!available) throw new TypeError('Connection lost');
    const { vote } = JSON.parse(String(options.body)) as { vote: MessageVote };
    saved = vote;
    return Response.json({ message_id: 'event:5', vote });
  });
  const view = render(draw());
  await waitFor(() =>
    expect(answer().getByRole('button', { name: 'Needs improvement' })).toBeEnabled(),
  );
  await userEvent.click(answer().getByRole('button', { name: 'Needs improvement' }));
  expect(await screen.findByRole('button', { name: 'Retry vote' })).toBeEnabled();
  view.unmount();
  render(draw());
  const retry = await screen.findByRole('button', { name: 'Retry vote' });
  expect(answer().getByRole('button', { name: 'Helpful' })).toHaveAttribute('aria-pressed', 'true');
  expect(answer().getByRole('button', { name: 'Helpful' })).toBeDisabled();
  available = true;
  await userEvent.click(retry);
  await waitFor(() =>
    expect(answer().getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
      'aria-pressed',
      'true',
    ),
  );
});

it('offers visible read recovery and restores the saved choice before allowing edits', async () => {
  let unavailable = true;
  vi.stubGlobal('fetch', async () =>
    unavailable
      ? Response.json({}, { status: 503 })
      : Response.json({ items: [{ message_id: 'event:5', vote: 'down' }], next: null }),
  );
  render(draw());
  const retry = await screen.findByRole('button', { name: 'Retry feedback' });
  expect(answer().getByRole('button', { name: 'Helpful' })).toBeDisabled();
  unavailable = false;
  await userEvent.click(retry);
  await waitFor(() =>
    expect(answer().getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
      'aria-pressed',
      'true',
    ),
  );
  expect(screen.queryByRole('button', { name: 'Retry feedback' })).toBeNull();
});

it('announces confirmed helpful, changed and removed votes without accepting a second in-flight choice', async () => {
  let complete!: () => void;
  vi.stubGlobal('fetch', async (_path: string, options?: RequestInit) => {
    if (options?.method !== 'PUT') return Response.json({ items: [], next: null });
    const { vote } = JSON.parse(String(options.body)) as { vote: MessageVote };
    await new Promise<void>((resolve) => {
      complete = resolve;
    });
    return Response.json({ message_id: 'event:5', vote });
  });
  render(draw());
  await waitFor(() => expect(answer().getByRole('button', { name: 'Helpful' })).toBeEnabled());
  for (const [label, selected, status] of [
    ['Helpful', true, 'Vote recorded'],
    ['Needs improvement', true, 'Vote recorded'],
    ['Needs improvement', false, 'Vote removed'],
  ] as const) {
    await userEvent.click(answer().getByRole('button', { name: label }));
    expect(answer().getByRole('status')).toHaveTextContent('Saving vote');
    expect(answer().getByRole('button', { name: 'Helpful' })).toBeDisabled();
    expect(answer().getByRole('button', { name: 'Needs improvement' })).toBeDisabled();
    complete();
    await waitFor(() => expect(answer().getByRole('status')).toHaveTextContent(status));
    expect(answer().getByRole('button', { name: label })).toHaveAttribute(
      'aria-pressed',
      String(selected),
    );
  }
});

it('keeps copy available without voting controls when access becomes read-only', async () => {
  vi.stubGlobal('fetch', async () =>
    Response.json({ items: [{ message_id: 'event:5', vote: 'up' }], next: null }),
  );
  const view = render(draw());
  await waitFor(() =>
    expect(answer().getByRole('button', { name: 'Helpful' })).toHaveAttribute(
      'aria-pressed',
      'true',
    ),
  );
  view.rerender(draw({ enabled: false }));
  expect(screen.queryByRole('button', { name: 'Helpful' })).toBeNull();
  expect(answer().getByRole('button', { name: 'Copy content' })).toBeEnabled();
});

it('offers feedback only on the recorded final answer and restores it when reopened', async () => {
  const votes = new Map<string, MessageVote>();
  vi.stubGlobal('fetch', async (path: string, options?: RequestInit) => {
    if (options?.method === 'PUT' && path.endsWith('/messages/event%3A5/feedback')) {
      const { vote } = JSON.parse(String(options.body)) as { vote: MessageVote };
      votes.set('event:5', vote);
      return Response.json({ message_id: 'event:5', vote });
    }
    if (path === '/api/v1/agents/agent/message-feedback?limit=200') {
      return Response.json({
        items: [...votes].map(([message_id, vote]) => ({ message_id, vote })),
        next: null,
      });
    }
    throw new Error(`Unexpected request: ${options?.method ?? 'GET'} ${path}`);
  });
  const view = render(draw());
  await waitFor(() => expect(answer().getByRole('button', { name: 'Helpful' })).toBeEnabled());
  expect(answer('Checking the evidence.').queryByRole('button', { name: 'Helpful' })).toBeNull();
  await userEvent.click(answer().getByRole('button', { name: 'Helpful' }));
  await waitFor(() =>
    expect(answer().getByRole('button', { name: 'Helpful' })).toHaveAttribute(
      'aria-pressed',
      'true',
    ),
  );
  view.unmount();
  render(draw());
  await waitFor(() =>
    expect(answer().getByRole('button', { name: 'Helpful' })).toHaveAttribute(
      'aria-pressed',
      'true',
    ),
  );
  expect(screen.getAllByRole('article')).toHaveLength(3);
});

it('waits for the durable successful turn outcome before offering feedback on live replies', async () => {
  vi.stubGlobal('fetch', async () => Response.json({ items: [], next: null }));
  const snapshot = { agentId: 'agent', throughSeq: 0, turns: [] };
  const events: AgentEvent[] = [
    {
      seq: 1,
      appendedAt: '2026-11-03T01:00:00Z',
      kind: { type: 'turn_started', turn_sequence: 1 },
    },
    {
      seq: 2,
      appendedAt: '2026-11-03T01:00:00Z',
      kind: { type: 'owner_message', content: 'Review the result.' },
    },
    {
      seq: 3,
      appendedAt: '2026-11-03T01:00:01Z',
      kind: { type: 'agent_message', content: 'Checking the evidence.' },
    },
    {
      seq: 5,
      appendedAt: '2026-11-03T01:00:02Z',
      kind: { type: 'agent_message', content: 'The final answer is ready.' },
    },
  ];
  const view = render(draw({ snapshot, events }));
  expect(screen.queryByRole('button', { name: 'Helpful' })).toBeNull();
  view.rerender(
    draw({
      snapshot,
      events: [
        ...events,
        {
          seq: 6,
          appendedAt: '2026-11-03T01:00:03Z',
          kind: { type: 'turn_completed', turn_sequence: 1 },
        },
      ],
    }),
  );
  await waitFor(() => expect(answer().getByRole('button', { name: 'Helpful' })).toBeEnabled());
  expect(answer('Checking the evidence.').queryByRole('button', { name: 'Helpful' })).toBeNull();
});
