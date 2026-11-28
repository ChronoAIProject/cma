import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript } from './conversation';
import type { ConversationMessage } from './model';
import type { MessageUser } from './MessageIdentity';

const user: MessageUser = {
  displayName: 'Ada Lovelace',
  handle: 'ada',
  avatarUrl: 'https://avatars.example/ada.png',
};
const messages: readonly ConversationMessage[] = [
  { id: 'owner', role: 'user', content: 'Recorded message', status: 'complete' },
  { id: 'reply', role: 'assistant', content: 'Recorded reply', status: 'complete' },
  { id: 'live', role: 'assistant', content: 'Incoming reply', status: 'streaming' },
];
const conversation = {
  ...conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
  messages,
};

function draw(identity?: MessageUser) {
  return (
    <MessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      user={identity}
      optimisticMessages={[{ id: 'pending', content: 'New message', state: 'sending' }]}
    />
  );
}

it('uses one personal identity for recorded and optimistic messages and one Agent mark per run', () => {
  render(draw(user));
  const own = screen.getAllByRole('article', { name: 'Ada Lovelace' });
  expect(own).toHaveLength(2);
  for (const row of own) {
    expect(within(row).getByText('Ada Lovelace')).toBeVisible();
    const avatar = row.querySelector('img')!;
    expect(avatar).toHaveAttribute('src', user.avatarUrl);
    expect(avatar).toHaveAttribute('alt', '');
    expect(avatar).toHaveAttribute('referrerpolicy', 'no-referrer');
    expect(avatar.parentElement).toHaveAttribute('aria-hidden', 'true');
  }
  expect(within(own[0]!).getByText('Recorded message')).toBeVisible();
  expect(within(own[1]!).getByRole('status')).toHaveTextContent('Sending…');
  const agents = screen.getAllByRole('article', { name: 'Chrono Agent' });
  expect(agents).toHaveLength(2);
  // The live reply continues the recorded one, so only the first draws the identity.
  expect(within(agents[1]!).queryByText('Chrono Agent')).toBeNull();
  expect(agents[1]!.querySelector('.message__avatar')).toHaveAttribute('data-continued');
  expect(agents[1]!.querySelector('.message__avatar')!.childElementCount).toBe(0);
  for (const row of agents.slice(0, 1)) {
    expect(within(row).getByText('Chrono Agent')).toBeVisible();
    const avatar = row.querySelector('.message__avatar')!;
    expect(avatar).toHaveAttribute('aria-hidden', 'true');
    const poster = avatar.querySelector('.agent-mascot__poster');
    expect(poster).toHaveAttribute('alt', '');
    expect(poster).toHaveAttribute('width', '32');
    expect(row.querySelector('canvas')).toBeNull();
  }
  expect(agents[1]).toHaveAttribute('data-streaming');
});

it('retains copy and timestamps without votes when no owned final answer is confirmed', () => {
  render(draw(user));
  for (const row of screen.getAllByRole('article', { name: 'Ada Lovelace' })) {
    expect(within(row).queryByRole('button', { name: 'Helpful' })).toBeNull();
    expect(within(row).queryByRole('button', { name: 'Needs improvement' })).toBeNull();
    expect(within(row).getByRole('button', { name: 'Copy content' })).toBeEnabled();
    expect(row.querySelector('time')).toBeNull();
  }
  for (const row of screen.getAllByRole('article', { name: 'Chrono Agent' })) {
    expect(within(row).getByRole('button', { name: 'Helpful' })).toBeDisabled();
    expect(within(row).getByRole('button', { name: 'Needs improvement' })).toBeDisabled();
    expect(within(row).getByRole('button', { name: 'Copy content' })).toBeEnabled();
  }
});

it('falls back after an image error and recovers when the account supplies a different image', () => {
  const view = render(draw(user));
  const avatar = screen.getAllByRole('article', { name: 'Ada Lovelace' })[0]!.querySelector('img')!;
  fireEvent.error(avatar);
  const row = screen.getAllByRole('article', { name: 'Ada Lovelace' })[0]!;
  expect(row.querySelector('img')).toBeNull();
  expect(within(row).getByText('AL')).toBeInTheDocument();
  view.rerender(draw({ ...user, avatarUrl: 'https://avatars.example/new.png' }));
  expect(row.querySelector('img')).toHaveAttribute('src', 'https://avatars.example/new.png');
});

it.each([
  undefined,
  '',
  'javascript:alert(1)',
  'data:image/svg+xml,unsafe',
  '//other.example/a.png',
  '/\\other.example/a.png',
  'https://name:password@other.example/a.png',
  'http://other.example/a.png',
])('uses initials when an avatar is missing or unsafe: %s', (avatarUrl) => {
  render(draw({ displayName: ' 王 小明 ', handle: 'wang', avatarUrl }));
  for (const row of screen.getAllByRole('article', { name: '王 小明' })) {
    expect(row.querySelector('img')).toBeNull();
    expect(within(row).getByText('王小')).toBeInTheDocument();
  }
});

it('uses the account handle for an empty display name and accepts a local avatar path', () => {
  render(draw({ displayName: ' ', handle: 'ada-lovelace', avatarUrl: '/avatar.png' }));
  for (const row of screen.getAllByRole('article', { name: 'ada-lovelace' }))
    expect(row.querySelector('img')).toHaveAttribute('src', '/avatar.png');
});

it('does not attribute unknown shared authors to the current viewer', () => {
  render(
    <MessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      unknownUserName="Participant"
    />,
  );
  expect(screen.getByRole('article', { name: 'Participant' })).toHaveTextContent(
    'Recorded message',
  );
  expect(screen.queryByRole('article', { name: 'You' })).not.toBeInTheDocument();
});

describe('identity runs', () => {
  const base = conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
  const named = (row: HTMLElement) => row.querySelector('.message__role')?.textContent ?? null;

  it('shows the identity once per run, across tool calls, and again after the other author', () => {
    render(
      <MessageStream
        conversation={{
          ...base,
          messages: [
            { order: 1, id: 'ask', role: 'user', content: 'Deploy it', status: 'complete' },
            { order: 2, id: 'more', role: 'user', content: 'Then report', status: 'complete' },
            { order: 3, id: 'first', role: 'assistant', content: 'Starting', status: 'complete' },
            { order: 5, id: 'second', role: 'assistant', content: 'Live now', status: 'complete' },
            { order: 6, id: 'thanks', role: 'user', content: 'Thanks', status: 'complete' },
          ],
          toolCalls: [{ order: 4, id: 'call', title: 'Check status', status: 'completed' }],
        }}
        onApproval={vi.fn()}
        user={user}
      />,
    );
    const own = screen.getAllByRole('article', { name: 'Ada Lovelace' });
    const agent = screen.getAllByRole('article', { name: 'Chrono Agent' });
    expect(own.map(named)).toEqual(['Ada Lovelace', null, 'Ada Lovelace']);
    expect(agent.map(named)).toEqual(['Chrono Agent', null]);
    expect(own[1]!.querySelector('img')).toBeNull();
    expect(agent[1]!.querySelector('.message__avatar img')).toBeNull();
    const process = screen.getByRole('button', { name: 'Check status Duration unavailable' });
    expect(process).toHaveAttribute('aria-expanded', 'false');
    fireEvent.click(process);
    expect(process).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('article', { name: 'Tool call: Check status' })).toBeInTheDocument();
  });

  it('continues a run into optimistic messages only when the last recorded message is the same author', () => {
    const view = render(
      <MessageStream
        conversation={{
          ...base,
          messages: [{ id: 'ask', role: 'user', content: 'Deploy it', status: 'complete' }],
        }}
        onApproval={vi.fn()}
        user={user}
        optimisticMessages={[
          { id: 'one', content: 'Also this', state: 'sending' },
          { id: 'two', content: 'And this', state: 'sending' },
        ]}
      />,
    );
    expect(screen.getAllByRole('article', { name: 'Ada Lovelace' }).map(named)).toEqual([
      'Ada Lovelace',
      null,
      null,
    ]);
    view.rerender(
      <MessageStream
        conversation={{ ...base, messages }}
        onApproval={vi.fn()}
        user={user}
        optimisticMessages={[
          { id: 'one', content: 'Also this', state: 'sending' },
          { id: 'two', content: 'And this', state: 'sending' },
        ]}
      />,
    );
    expect(screen.getAllByRole('article', { name: 'Ada Lovelace' }).map(named)).toEqual([
      'Ada Lovelace',
      'Ada Lovelace',
      null,
    ]);
  });
});
