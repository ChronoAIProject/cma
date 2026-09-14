import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MARKETPLACE_SANDBOX, marketplaceClient } from '../../test/profiles';
import { Marketplace } from './Marketplace';

it('pages marketplace cards, searches publicly and navigates to detail with readable metadata', async () => {
  const actor = userEvent.setup();
  const listMarketplace = vi
    .fn()
    .mockResolvedValueOnce({ items: [MARKETPLACE_SANDBOX.summary], next: 'page-two' })
    .mockResolvedValueOnce({
      items: [{ ...MARKETPLACE_SANDBOX.summary, id: 'sbp_second', name: 'Second sandbox' }],
    })
    .mockResolvedValue({ items: [] });
  const onNavigate = vi.fn();
  render(<Marketplace client={{ listMarketplace }} onNavigate={onNavigate} />);
  await actor.click(await screen.findByRole('button', { name: 'More profiles' }));
  expect(await screen.findByRole('link', { name: 'Second sandbox' })).toBeInTheDocument();
  expect(screen.getAllByText('By Research team')).toHaveLength(2);
  expect(screen.getAllByText('12 uses')).toHaveLength(2);
  expect(listMarketplace.mock.calls[1]?.[2].cursor).toBe('page-two');
  await actor.click(screen.getByRole('link', { name: 'Research sandbox' }));
  expect(onNavigate).toHaveBeenCalledWith({
    kind: 'marketplace_profile',
    profileKind: 'sandbox',
    profileId: MARKETPLACE_SANDBOX.summary.id,
  });
  fireEvent.change(screen.getByLabelText('Search marketplace'), { target: { value: 'missing' } });
  expect(await screen.findByText('No matching profiles')).toBeInTheDocument();
  expect(listMarketplace).toHaveBeenLastCalledWith(
    'sandbox',
    'missing',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
});

it('aborts a previous tab request and ignores its late result', async () => {
  const actor = userEvent.setup();
  let resolve: ((value: unknown) => void) | undefined;
  const listMarketplace = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((settle) => {
          resolve = settle;
        }),
    )
    .mockResolvedValue({ items: [] });
  render(<Marketplace client={{ listMarketplace }} onNavigate={vi.fn()} />);
  await waitFor(() => expect(listMarketplace).toHaveBeenCalledTimes(1));
  await actor.click(screen.getByRole('tab', { name: 'Agent' }));
  expect(listMarketplace.mock.calls[0]?.[2].signal.aborted).toBe(true);
  await act(async () => {
    resolve?.({ items: [MARKETPLACE_SANDBOX.summary] });
  });
  await screen.findByText('No matching profiles');
  expect(screen.queryByRole('link', { name: 'Research sandbox' })).not.toBeInTheDocument();
});

it('explains when the deployment has disabled the marketplace', async () => {
  render(
    <Marketplace
      client={marketplaceClient({
        listMarketplace: vi.fn().mockRejectedValue({ status: 503, code: 'marketplace_disabled' }),
      })}
      onNavigate={vi.fn()}
    />,
  );
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'The marketplace is currently unavailable.',
  );
});
