import { act, fireEvent, render, screen } from '@testing-library/react';
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
