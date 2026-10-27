import { render, screen, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { ChannelActivityView } from './ChannelActivity';
import { ChannelActivityPreview } from './ChannelActivityPreview';
import type { ChannelActivityEvent, ChannelActivityFeed } from './useChannelActivity';

const receipt: ChannelActivityEvent = {
  event_id: 'deleted-message',
  kind: 'event',
  event_type: 'cma.message.deleted.v1',
  source: { type: 'discord', bot_id: '123' },
  actor: { id: 'unknown', kind: 'unknown' },
  conversation: { id: 'conversation', kind: 'thread', thread_id: 'thread' },
  occurred_at: '2026-10-27T08:00:00Z',
  received_at: '2026-10-27T08:00:01Z',
  consumers: [],
};
const feed = (event: ChannelActivityEvent): ChannelActivityFeed => ({
  page: { items: [event], cursor: 'cursor', limit: 50, retention_days: 7, has_more: false },
  connection: 'live',
  retry: () => undefined,
});

it('labels a deletion in the map preview and details without inventing actor or source time', () => {
  render(
    <>
      <ChannelActivityPreview feed={feed(receipt)} />
      <ChannelActivityView feed={feed(receipt)} />
    </>,
  );
  const preview = screen.getByRole('region', { name: 'Latest channel activity' });
  expect(within(preview).getByText('Message deleted · discord')).toBeVisible();
  expect(within(preview).getByText('Actor not reported')).toBeVisible();
  expect(screen.getByText('Source event time').nextElementSibling).toHaveTextContent(
    'Not reported',
  );
  expect(document.querySelector('time[datetime="2026-10-27T08:00:00Z"]')).toBeNull();
  expect(document.querySelectorAll('time[datetime="2026-10-27T08:00:01Z"]')).toHaveLength(2);
  expect(screen.queryByText(/Message received/)).not.toBeInTheDocument();
});

it('updates event facts in place and retains a generic label for old receipts', () => {
  const { rerender } = render(<ChannelActivityView feed={feed(receipt)} />);
  rerender(
    <ChannelActivityView
      feed={feed({
        ...receipt,
        event_type: 'cma.message.updated.v1',
        source_occurred_at: '2026-10-27T07:59:00Z',
        actor: { id: 'person', display_name: 'Alex', kind: 'human' },
      })}
    />,
  );
  expect(screen.getByText('Message edited · discord')).toBeVisible();
  expect(screen.getByText('Alex ·', { exact: false })).toBeVisible();
  expect(document.querySelector('time[datetime="2026-10-27T07:59:00Z"]')).toBeVisible();
  expect(screen.queryByText(/Message deleted/)).not.toBeInTheDocument();
  rerender(
    <ChannelActivityView feed={feed({ ...receipt, kind: undefined, event_type: undefined })} />,
  );
  expect(screen.getByText('Event received · discord')).toBeVisible();
  expect(screen.getByText('Recorded event time')).toBeVisible();
  expect(screen.queryByText(/Message received/)).not.toBeInTheDocument();
});

it('shows reaction changes using the same receipt layout', () => {
  render(
    <ChannelActivityPreview
      feed={feed({ ...receipt, event_type: 'cma.message.reaction.updated.v1' })}
    />,
  );
  expect(screen.getByText('Reaction updated · discord')).toBeVisible();
});
