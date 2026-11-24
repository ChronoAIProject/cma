import { fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext } from '../RelayCapabilities';
import { ChannelEditor } from '../ChannelEditor';

afterEach(() => vi.unstubAllGlobals());

it('keeps required Agent identity visible while optional expert controls stay collapsed', () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  const owner = screen.getByLabelText('Owner subject');
  expect(owner.closest('details')).toHaveAttribute('open');
  expect(owner).toHaveAccessibleDescription('Required.');
  expect(screen.getByText('Advanced model connection').closest('details')).not.toHaveAttribute(
    'open',
  );
  expect(screen.getByLabelText('Service scope 1')).toBeVisible();
  expect(screen.getByLabelText('Permission mode')).toBeVisible();
  const access = within(screen.getByRole('group', { name: 'Access & reply policy' }));
  expect(access.getByRole('button', { name: 'Admission policy' })).toHaveTextContent(
    'All messages',
  );
  expect(access.getByRole('button', { name: 'Group incoming messages by' })).toHaveTextContent(
    'Each sender in each conversation',
  );
  expect(fetcher).not.toHaveBeenCalled();
});

it('reveals a newly added required expert field and preserves the draft across disclosure changes', () => {
  render(<ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />);
  const details = screen.getByText('Advanced Agent tools and environment').closest('details')!;
  fireEvent.click(screen.getByText('Advanced Agent tools and environment'));
  fireEvent.click(screen.getByRole('button', { name: 'Add skill' }));
  const skill = screen.getByLabelText('Skill 1 ID');
  expect(skill).toBeRequired();
  expect(details).toHaveAttribute('open');
  fireEvent.change(skill, { target: { value: 'local-skill' } });
  fireEvent.click(screen.getByText('Advanced Agent tools and environment'));
  expect(details).not.toHaveAttribute('open');
  fireEvent.change(screen.getByLabelText('Channel name'), {
    target: { value: 'Retained channel' },
  });
  fireEvent.click(screen.getByText('Advanced Agent tools and environment'));
  expect(screen.getByLabelText('Skill 1 ID')).toHaveValue('local-skill');
  expect(
    within(screen.getByRole('group', { name: 'Review' })).getByText('Retained channel'),
  ).toBeVisible();
});

it('retains HMAC credential editing for managed connections with webhook replies', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [] }))));
  render(
    <RelayCapabilitiesContext value={{ status: 'ready', managedTelegramSetup: true }}>
      <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Use an existing CMA-managed Telegram connection'));
  await screen.findByText(/No saved CMA connections/);
  fireEvent.click(screen.getByRole('button', { name: 'Destination 1 type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Result webhook' }));
  expect(screen.getByRole('button', { name: 'Add HMAC credential' })).toBeVisible();
  expect(screen.queryByLabelText('Replace HMAC credentials')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Provider agent key')).toBeRequired();
  expect(screen.queryByLabelText('Telegram bot token')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('NyxID channel key')).not.toBeInTheDocument();
});

it.each(['loading', 'legacy', 'failed', 'ready'] as const)(
  'hides new managed setup until explicitly advertised (%s)',
  (status) => {
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    render(
      <RelayCapabilitiesContext value={{ status }}>
        <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
      </RelayCapabilitiesContext>,
    );
    expect(
      screen.queryByLabelText('Use an existing CMA-managed Telegram connection'),
    ).not.toBeInTheDocument();
    expect(fetcher).not.toHaveBeenCalled();
  },
);

it('retains a selected managed draft when the capability becomes unavailable', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [] }))));
  const view = (enabled: boolean) => (
    <RelayCapabilitiesContext value={{ status: 'ready', managedTelegramSetup: enabled }}>
      <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const mounted = render(view(true));
  fireEvent.click(screen.getByLabelText('Use an existing CMA-managed Telegram connection'));
  await screen.findByText(/No saved CMA connections/);
  mounted.rerender(view(false));
  expect(screen.getByLabelText('Use an existing CMA-managed Telegram connection')).toBeChecked();
  expect(screen.getByRole('button', { name: 'Saved Telegram connection' })).toBeVisible();
});
