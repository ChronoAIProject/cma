import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { MessageFooter, MessageTimestamp } from './MessageFooter';
import { useMessageFeedback } from './useMessageFeedback';
import type { CmaWebClient, MessageFeedback, MessageVote } from '../api/client';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('keeps a semantic timestamp and refreshes relative labels at their boundaries', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-05T00:00:59Z'));
  const view = render(<MessageTimestamp at="2026-11-05T00:00:00Z" />);
  const time = screen.getByText('just now');
  expect(time.tagName).toBe('TIME');
  expect(time).toHaveAttribute('datetime', '2026-11-05T00:00:00Z');
  expect(time).toHaveAttribute('aria-live', 'off');
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(time).toHaveTextContent('1 minute ago');
  await act(() => vi.advanceTimersByTimeAsync(240000));
  expect(time).toHaveTextContent('5 minutes ago');
  vi.setSystemTime(new Date('2026-11-05T03:00:00Z'));
  await act(async () => window.dispatchEvent(new Event('focus')));
  expect(time).toHaveTextContent('3 hours ago');
  vi.setSystemTime(new Date('2026-11-07T00:00:00Z'));
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  expect(time).toHaveTextContent('2 days ago');
  view.rerender(<MessageTimestamp at="invalid" />);
  expect(view.container.querySelector('time')).toBeNull();
  view.rerender(<MessageTimestamp />);
  expect(view.container).toBeEmptyDOMElement();
});

it('places the three persistent action icons before time and omits obsolete footer details', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-05T00:05:00Z'));
  const { container } = render(
    <MessageFooter id="event:1" canVote committed content="Answer" at="2026-11-05T00:00:00Z" />,
  );
  const footer = container.querySelector('footer')!;
  expect(
    within(footer)
      .getAllByRole('button')
      .map((button) => button.getAttribute('aria-label')),
  ).toEqual(['Copy content', 'Helpful', 'Needs improvement']);
  const actions = footer.querySelector('.message-footer__actions')!;
  expect(actions.nextElementSibling?.tagName).toBe('TIME');
  expect(actions.nextElementSibling).toHaveTextContent('5 minutes ago');
  expect(footer).not.toHaveTextContent(
    /SGT|Worked for|input tokens|output tokens|View plan|Watch replay/,
  );
  expect(within(footer).getByRole('button', { name: 'Helpful' })).toBeDisabled();
});

it('copies actual content, acknowledges success, and handles clipboard failure without changing text', async () => {
  const actor = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  render(
    <article className="message">
      <MessageFooter id="live" canVote content={'**Exact**\nmessage'} committed={false} />
    </article>,
  );
  const button = screen.getByRole('button', { name: 'Copy content' });
  button.focus();
  expect(button).toHaveFocus();
  await actor.click(button);
  expect(copy).toHaveBeenCalledWith('**Exact**\nmessage');
  expect(screen.getByRole('status')).toHaveTextContent('Copied');
  expect(screen.getByRole('button', { name: 'Helpful' })).toBeDisabled();
  copy.mockRejectedValueOnce(new Error('Permission denied'));
  await actor.click(button);
  expect(screen.getByRole('status')).toHaveTextContent('Copy failed.');
});

function FeedbackMessages({
  owner = 'owner',
  enabled = true,
}: {
  readonly owner?: string;
  readonly enabled?: boolean;
}) {
  const feedback = useMessageFeedback('agent', owner, enabled);
  return (
    <>
      {['event:12', 'event:16'].map((id) => (
        <article key={id} aria-label={id} className="message">
          <MessageFooter
            id={id}
            canVote
            at="2026-10-20T00:00:00Z"
            content={id}
            committed
            feedback={feedback}
          />
        </article>
      ))}
    </>
  );
}
function wrapped(client: CmaWebClient, owner = 'owner', enabled = true) {
  return (
    <CmaClientProvider client={client}>
      <FeedbackMessages owner={owner} enabled={enabled} />
    </CmaClientProvider>
  );
}

it('persists exact-message votes, switches/toggles them, and restores votes on remount without starting work', async () => {
  const stored = new Map<string, MessageVote>([['event:12', 'up']]);
  const set = vi.fn(async (_agent: string, message_id: string, vote: MessageVote) => {
    stored.set(message_id, vote);
    return { message_id, vote };
  });
  const client = agentClient({
    readMessageFeedback: async () =>
      [...stored].map(([message_id, vote]) => ({ message_id, vote })),
    setMessageFeedback: set,
    runAgent: vi.fn(),
  });
  const view = render(wrapped(client));
  const first = within(screen.getByRole('article', { name: 'event:12' }));
  const second = within(screen.getByRole('article', { name: 'event:16' }));
  await waitFor(() =>
    expect(first.getByRole('button', { name: 'Helpful' })).toHaveAttribute('aria-pressed', 'true'),
  );
  await userEvent.click(first.getByRole('button', { name: 'Needs improvement' }));
  expect(set).toHaveBeenLastCalledWith(
    'agent',
    'event:12',
    'down',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(first.getByRole('button', { name: 'Helpful' })).toHaveAttribute('aria-pressed', 'false');
  expect(first.getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(second.getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  await userEvent.click(first.getByRole('button', { name: 'Needs improvement' }));
  expect(set).toHaveBeenLastCalledWith('agent', 'event:12', null, expect.anything());
  await userEvent.click(second.getByRole('button', { name: 'Helpful' }));
  view.unmount();
  render(wrapped(client));
  await waitFor(() =>
    expect(
      within(screen.getByRole('article', { name: 'event:16' })).getByRole('button', {
        name: 'Helpful',
      }),
    ).toHaveAttribute('aria-pressed', 'true'),
  );
  expect(client.runAgent).not.toHaveBeenCalled();
});

it('retains the confirmed vote after failure and retries the same requested value independently', async () => {
  const set = vi
    .fn()
    .mockRejectedValueOnce(new Error('Unavailable'))
    .mockResolvedValue({ message_id: 'event:12', vote: 'down' });
  const client = agentClient({
    readMessageFeedback: async () => [{ message_id: 'event:12', vote: 'up' }],
    setMessageFeedback: set,
  });
  render(wrapped(client));
  const first = within(screen.getByRole('article', { name: 'event:12' }));
  await waitFor(() =>
    expect(first.getByRole('button', { name: 'Needs improvement' })).toBeEnabled(),
  );
  await userEvent.click(first.getByRole('button', { name: 'Needs improvement' }));
  expect(first.getByRole('button', { name: 'Helpful' })).toHaveAttribute('aria-pressed', 'true');
  expect(first.getByRole('status')).toHaveTextContent('Unavailable');
  expect(
    within(screen.getByRole('article', { name: 'event:16' })).queryByRole('status'),
  ).toBeNull();
  await userEvent.click(first.getByRole('button', { name: 'Retry vote' }));
  expect(set.mock.calls[1]?.slice(0, 3)).toEqual(set.mock.calls[0]?.slice(0, 3));
  expect(first.getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
});

it('aborts old-account writes, hides old votes immediately, and refuses feedback on read-only views', async () => {
  let finish!: (value: MessageFeedback) => void;
  const set = vi.fn().mockImplementation(
    () =>
      new Promise<MessageFeedback>((resolve) => {
        finish = resolve;
      }),
  );
  const read = vi
    .fn()
    .mockResolvedValueOnce([{ message_id: 'event:12', vote: 'up' }])
    .mockResolvedValue([]);
  const client = agentClient({ readMessageFeedback: read, setMessageFeedback: set });
  const view = render(wrapped(client));
  const first = within(screen.getByRole('article', { name: 'event:12' }));
  await waitFor(() =>
    expect(first.getByRole('button', { name: 'Needs improvement' })).toBeEnabled(),
  );
  await userEvent.click(first.getByRole('button', { name: 'Needs improvement' }));
  view.rerender(wrapped(client, 'other'));
  expect(first.getByRole('button', { name: 'Helpful' })).toHaveAttribute('aria-pressed', 'false');
  expect(set.mock.calls[0]?.[3].signal.aborted).toBe(true);
  await act(async () => finish({ message_id: 'event:12', vote: 'down' }));
  expect(first.getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  view.rerender(wrapped(client, 'other', false));
  expect(first.getByRole('button', { name: 'Helpful' })).toBeDisabled();
  expect(first.getByRole('button', { name: 'Copy content' })).toBeEnabled();
});
