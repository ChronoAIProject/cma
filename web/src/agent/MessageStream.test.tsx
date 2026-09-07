import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import { MessageStream } from './MessageStream';

function conversation() {
  const events = [
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
  ];
  return events.reduce(
    (state, event) => foldLiveEvent(state, { ...event, runId: 'run', threadId: 'agent' }, 'agent'),
    conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
  );
}

it('renders messages and Agent activity in their original stream order', () => {
  render(<MessageStream conversation={conversation()} onApproval={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: /^Thinking/ }));
  const ordered = [
    'Find the issue',
    'Thinking',
    'Tool - Read package - running - ongoing',
    'Write package',
    'The issue is fixed',
  ].map((name) => screen.getByText(name));
  ordered
    .slice(1)
    .forEach((element, index) =>
      expect(
        ordered[index]!.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy(),
    );
});

it('follows new content only while the reader stays at the end', () => {
  const scrollIntoView = vi.fn();
  const previous = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = scrollIntoView;
  try {
    const draw = () => (
      <div className="agent-surface__conversation" data-testid="scroller">
        <MessageStream conversation={conversation()} onApproval={vi.fn()} />
      </div>
    );
    const { rerender } = render(draw());
    expect(scrollIntoView).toHaveBeenCalledOnce();
    const scroller = screen.getByTestId('scroller');
    Object.defineProperties(scroller, {
      scrollHeight: { value: 1000 },
      clientHeight: { value: 400 },
    });
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    rerender(draw());
    expect(scrollIntoView).toHaveBeenCalledOnce();
    scroller.scrollTop = 600;
    fireEvent.scroll(scroller);
    rerender(draw());
    expect(scrollIntoView).toHaveBeenCalledTimes(2);
  } finally {
    HTMLElement.prototype.scrollIntoView = previous;
  }
});
