import { render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import Markdown from 'react-markdown';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import type { ConversationState } from './model';
import { MessageStream } from './MessageStream';
import type { AgentFilesClient } from '../api/agentFiles';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { useMessageFeedback } from './useMessageFeedback';

vi.mock('react-markdown', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react-markdown')>();
  return { ...actual, default: vi.fn(actual.default) };
});
afterEach(() => vi.clearAllMocks());

it('parses only the changed reply while streaming over a long Markdown history', () => {
  let conversation: ConversationState = {
    ...conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
    messages: Array.from({ length: 200 }, (_, index) => ({
      id: `event:${index + 1}`,
      runId: 'old',
      role: 'assistant',
      status: 'complete',
      content: `**Recorded ${index}**\n\n- Preserved history`,
      order: index + 1,
    })),
  };
  const files = {} as AgentFilesClient;
  const draw = (workingDirectory = '/home/cma/first') => (
    <MessageStream
      conversation={conversation}
      media={{ agentId: 'agent', files, workingDirectory }}
      onApproval={vi.fn()}
    />
  );
  const view = render(draw());
  expect(Markdown).toHaveBeenCalledTimes(200);
  const historicalNode = screen.getByText('Recorded 0').closest('article');
  conversation = foldLiveEvent(
    conversation,
    {
      type: 'TEXT_MESSAGE_START',
      threadId: 'agent',
      runId: 'live',
      messageId: 'reply',
      role: 'assistant',
    },
    'agent',
  );
  view.rerender(draw());
  vi.mocked(Markdown).mockClear();
  for (let index = 0; index < 10; index++) {
    conversation = foldLiveEvent(
      conversation,
      {
        type: 'TEXT_MESSAGE_CONTENT',
        threadId: 'agent',
        runId: 'live',
        messageId: 'reply',
        delta: 'New text. ',
      },
      'agent',
    );
    view.rerender(draw());
  }
  expect(Markdown).toHaveBeenCalledTimes(10);
  expect(screen.getByText('Recorded 0').closest('article')).toBe(historicalNode);
  expect(view.container.querySelector('[data-message-key="message:reply"]')).toHaveTextContent(
    'New text. '.repeat(10).trim(),
  );
  vi.mocked(Markdown).mockClear();
  view.rerender(draw('/home/cma/second'));
  expect(Markdown).toHaveBeenCalledTimes(201);
});

it('keeps feedback controls stable across stream rerenders and revokes the old owner callbacks', async () => {
  const setVote = vi.fn();
  const client = {
    ...agentClient(),
    readMessageFeedback: vi.fn(async () => []),
    setMessageFeedback: setVote,
  };
  const hook = renderHook(({ owner }) => useMessageFeedback('agent', owner, true), {
    initialProps: { owner: 'owner' },
    wrapper: ({ children }) => <CmaClientProvider client={client}>{children}</CmaClientProvider>,
  });
  await waitFor(() => expect(hook.result.current?.ready).toBe(true));
  const before = hook.result.current;
  hook.rerender({ owner: 'owner' });
  expect(hook.result.current).toBe(before);
  hook.rerender({ owner: 'other' });
  expect(hook.result.current).not.toBe(before);
  before?.setVote('event:1', 'up');
  expect(setVote).not.toHaveBeenCalled();
  await waitFor(() => expect(hook.result.current?.ready).toBe(true));
});
