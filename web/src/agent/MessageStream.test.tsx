import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import { MessageStream } from './MessageStream';
import { AgentViewPositions } from './viewPosition';

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

it('restores the reader position for a revisited Agent instead of jumping to its newest message', () => {
  const positions = new AgentViewPositions();
  const viewPosition = positions.forAgent('one')!;
  const draw = () => (
    <div className="agent-surface__conversation" data-testid="scroller">
      <MessageStream
        conversation={conversation()}
        onApproval={vi.fn()}
        viewPosition={viewPosition}
      />
    </div>
  );
  const first = render(draw());
  const scroller = screen.getByTestId('scroller');
  Object.defineProperties(scroller, {
    scrollHeight: { value: 2000 },
    clientHeight: { value: 400 },
  });
  scroller.scrollTop = 1600;
  fireEvent.scroll(scroller);
  scroller.scrollTop = 300;
  fireEvent.scroll(scroller);
  first.unmount();
  expect(positions.forAgent('two')).toEqual({ top: 0, pinned: true });
  expect(positions.forAgent('one')).toBe(viewPosition);
  render(draw());
  expect(screen.getByTestId('scroller').scrollTop).toBe(300);
});

it('renders messages and Agent activity in their original stream order', () => {
  render(<MessageStream conversation={conversation()} onApproval={vi.fn()} />);
  for (const toggle of screen.getAllByRole('button', { name: /^1 tool call/ }))
    fireEvent.click(toggle);
  expect(screen.getAllByRole('button', { name: /^Thinking/ })).toHaveLength(1);
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
