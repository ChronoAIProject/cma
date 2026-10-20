import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import { MessageStream } from './MessageStream';

function conversation() {
  const events = [
    { type: 'RUN_STARTED' },
    { type: 'TEXT_MESSAGE_START', messageId: 'owner', role: 'user' },
    { type: 'TEXT_MESSAGE_CONTENT', messageId: 'owner', delta: 'Find the issue' },
    { type: 'REASONING_MESSAGE_CONTENT', messageId: 'thinking', delta: 'Inspect dependencies' },
    { type: 'TOOL_CALL_START', toolCallId: 'tool', toolCallName: 'Read package' },
    {
      type: 'STATE_DELTA',
      delta: [
        {
          op: 'add',
          path: '/pendingApprovals/approval',
          value: { id: 'approval', reason: 'Write package' },
        },
      ],
    },
    { type: 'TEXT_MESSAGE_START', messageId: 'assistant', role: 'assistant' },
    { type: 'TEXT_MESSAGE_CONTENT', messageId: 'assistant', delta: 'The issue is fixed' },
    { type: 'TOOL_CALL_END', toolCallId: 'tool' },
  ];
  return events.reduce(
    (state, event) => foldLiveEvent(state, { ...event, runId: 'run', threadId: 'agent' }, 'agent'),
    conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
  );
}

it('renders messages and Agent activity in their original stream order', () => {
  render(<MessageStream conversation={conversation()} onApproval={vi.fn()} />);
  expect(screen.queryByRole('button', { name: /^Thinking/ })).not.toBeInTheDocument();
  const ordered = ['Find the issue', 'Write package', 'The issue is fixed', 'Read package'].map(
    (name) => screen.getByText(name),
  );
  ordered
    .slice(1)
    .forEach((element, index) =>
      expect(
        ordered[index]!.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy(),
    );
});

it('keeps message times visible in SGT and exposes copy and feedback actions on demand', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText },
  });
  const transcript = {
    agentId: 'agent',
    throughSeq: 2,
    turns: [
      {
        turnSequence: 1,
        startedAt: '2026-09-06T00:01:00Z',
        ownerMessage: 'Inspect this repository.',
        agentMessages: ['The repository is ready.'],
        messages: [
          {
            sequence: 1,
            role: 'user' as const,
            content: 'Inspect this repository.',
            appendedAt: '2026-09-06T00:01:00Z',
          },
          {
            sequence: 2,
            role: 'assistant' as const,
            content: 'The repository is ready.',
            appendedAt: '2026-09-06T00:01:01Z',
          },
        ],
        outcome: { type: 'completed' as const },
      },
    ],
  };
  render(
    <MessageStream conversation={conversationFromTranscript(transcript)} onApproval={vi.fn()} />,
  );
  expect(screen.getAllByText('06 Sept 2026, 08:01 SGT')).toHaveLength(2);
  const copy = screen.getAllByRole('button', { name: 'Copy message' });
  expect(copy).toHaveLength(2);
  expect(screen.getAllByRole('button', { name: 'Upvote message' })).toHaveLength(2);
  expect(screen.getAllByRole('button', { name: 'Downvote message' })).toHaveLength(2);
  await userEvent.click(copy[1]!);
  expect(writeText).toHaveBeenCalledWith('The repository is ready.');
  const upvote = screen.getAllByRole('button', { name: 'Upvote message' })[1]!;
  await userEvent.click(upvote);
  expect(upvote).toHaveAttribute('aria-pressed', 'true');
});

it('follows new content only while the reader stays at the end', () => {
  let resize: (() => void) | undefined;
  const disconnect = vi.fn();
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(callback: () => void) {
        resize = callback;
      }
      observe() {}
      disconnect = disconnect;
    },
  );
  try {
    const draw = () => (
      <div className="agent-surface__conversation" data-testid="scroller">
        <MessageStream conversation={conversation()} onApproval={vi.fn()} />
      </div>
    );
    const { rerender, unmount } = render(draw());
    const scroller = screen.getByTestId('scroller');
    let height = 1000;
    Object.defineProperties(scroller, {
      scrollHeight: { get: () => height },
      clientHeight: { value: 400 },
    });
    act(() => resize?.());
    expect(scroller.scrollTop).toBe(1000);
    height = 1200;
    fireEvent.scroll(scroller);
    act(() => resize?.());
    expect(scroller.scrollTop).toBe(1200);
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    rerender(draw());
    act(() => resize?.());
    expect(scroller.scrollTop).toBe(0);
    scroller.scrollTop = 800;
    fireEvent.scroll(scroller);
    height = 1400;
    rerender(draw());
    expect(scroller.scrollTop).toBe(1400);
    unmount();
    expect(disconnect).toHaveBeenCalledOnce();
  } finally {
    vi.unstubAllGlobals();
  }
});
