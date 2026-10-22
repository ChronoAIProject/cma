import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
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

it('uses one personal identity for recorded and optimistic messages and one Agent mark for replay and live text', () => {
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
  for (const row of agents) {
    expect(within(row).getByText('Chrono Agent')).toBeVisible();
    // The mark is inline SVG painted with currentColor, never a raster with its own ground.
    const avatar = row.querySelector('.message__avatar')!;
    expect(avatar).toHaveAttribute('aria-hidden', 'true');
    expect(avatar.querySelector('img')).toBeNull();
    expect(avatar.querySelector('svg')).not.toBeNull();
    for (const painted of avatar.querySelectorAll('[fill], [stroke]')) {
      for (const attribute of ['fill', 'stroke']) {
        const value = painted.getAttribute(attribute);
        if (value !== null) expect(['currentColor', 'none']).toContain(value);
      }
    }
    expect(row.querySelector('canvas')).toBeNull();
  }
  expect(agents[1]).toHaveAttribute('data-streaming');
});

it('offers votes only for Agent replies while retaining copy and timestamps on every message', () => {
  render(draw(user));
  for (const row of screen.getAllByRole('article', { name: 'Ada Lovelace' })) {
    expect(within(row).queryByRole('button', { name: 'Upvote message' })).toBeNull();
    expect(within(row).queryByRole('button', { name: 'Downvote message' })).toBeNull();
    expect(within(row).getByRole('button', { name: 'Copy content' })).toBeEnabled();
    expect(within(row).getByText('Time unavailable')).toBeInTheDocument();
  }
  for (const row of screen.getAllByRole('article', { name: 'Chrono Agent' })) {
    expect(within(row).getByRole('button', { name: 'Upvote message' })).toBeInTheDocument();
    expect(within(row).getByRole('button', { name: 'Downvote message' })).toBeInTheDocument();
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
