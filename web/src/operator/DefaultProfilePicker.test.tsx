import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { createOperatorClient } from './client';
import { DefaultProfilePicker } from './DefaultProfilePicker';
import { operatorClient, publicProfile } from './testFixtures';

describe('operator public default picker', () => {
  it('pages and searches public profiles without silently changing a pinned choice', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const change = vi.fn();
    const profile = publicProfile('agent');
    client.listPublicProfiles
      .mockResolvedValueOnce({ items: [profile], next: 'page-2' })
      .mockResolvedValueOnce({ items: [profile, { ...profile, id: 'second', latest_revision: 8 }] })
      .mockResolvedValue({ items: [profile] });
    render(
      <DefaultProfilePicker
        client={client}
        kind="agent"
        value={{ id: 'pinned', revision: 3 }}
        disabled={false}
        onChange={change}
      />,
    );
    await user.click(await screen.findByRole('button', { name: 'More profiles' }));
    await user.click(screen.getByRole('button', { name: 'Default agent profile' }));
    await screen.findByRole('menuitemradio', { name: 'Public agent / revision 8 / Owner' });
    expect(screen.getAllByRole('menuitemradio')).toHaveLength(3);
    expect(change).not.toHaveBeenCalled();
    expect(client.listPublicProfiles).toHaveBeenLastCalledWith(
      'agent',
      expect.objectContaining({ cursor: 'page-2' }),
    );
    await user.click(
      screen.getByRole('menuitemradio', { name: 'Public agent / revision 8 / Owner' }),
    );
    expect(change).toHaveBeenCalledWith({ id: 'second', revision: 8 });
    await user.click(screen.getByRole('button', { name: 'Refresh agent profiles' }));
    await waitFor(() =>
      expect(client.listPublicProfiles).toHaveBeenLastCalledWith(
        'agent',
        expect.objectContaining({ cursor: undefined }),
      ),
    );
    await user.type(screen.getByLabelText('Search public agent profiles'), 'review');
    await user.click(screen.getByRole('button', { name: 'Search agent profiles' }));
    await waitFor(() =>
      expect(client.listPublicProfiles).toHaveBeenLastCalledWith(
        'agent',
        expect.objectContaining({ q: 'review', cursor: undefined }),
      ),
    );
    expect(screen.getByRole('button', { name: 'Default agent profile' })).toHaveTextContent(
      'Current profile / revision 3',
    );
    expect(change).toHaveBeenCalledTimes(1);
  });

  it('binds the operator adapter to the shared admin client and kind-specific marketplace', async () => {
    const sdk = createClient();
    const page = { items: [publicProfile('agent')] };
    const list = vi.spyOn(sdk.marketplace.agent, 'list').mockResolvedValue(page);
    const client = createOperatorClient(sdk);
    expect(client.admin).toBe(sdk.admin);
    expect(await client.listPublicProfiles('agent', { q: 'review' })).toBe(page);
    expect(list).toHaveBeenCalledWith({ q: 'review' });
  });
});
