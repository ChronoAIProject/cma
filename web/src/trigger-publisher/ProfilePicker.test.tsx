import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { PublisherProfileChoice } from './client';
import { profiles, publisherFixture } from './fixtures';
import { ProfilePicker } from './ProfilePicker';

describe('publisher profile pagination', () => {
  it('ignores an old source pagination response after switching sources', async () => {
    const user = userEvent.setup();
    const test = publisherFixture();
    let finish: ((value: { items: PublisherProfileChoice[] }) => void) | undefined;
    vi.mocked(test.client.listProfiles)
      .mockResolvedValueOnce({ items: [profiles.agent], next: 'more' })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      )
      .mockResolvedValueOnce({
        items: [{ ...profiles.agent, id: 'public-agent', name: 'Public agent' }],
      });
    render(<ProfilePicker client={test.client} kind="agent" disabled={false} onSelect={vi.fn()} />);
    await user.click(await screen.findByRole('button', { name: 'Load more' }));
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'agent profile source' }),
      'public',
    );
    await screen.findByRole('option', { name: 'Public agent / Revision 4' });
    await act(async () =>
      finish?.({ items: [{ ...profiles.agent, id: 'stale-agent', name: 'Stale agent' }] }),
    );
    expect(
      screen.queryByRole('option', { name: 'Stale agent / Revision 4' }),
    ).not.toBeInTheDocument();
  });
  it('returns to own Workspace profiles when a different Sandbox has no references', async () => {
    const test = publisherFixture();
    const props = {
      client: test.client,
      kind: 'workspace' as const,
      disabled: false,
      onSelect: vi.fn(),
    };
    const view = render(<ProfilePicker {...props} references={[profiles.workspace]} />);
    await screen.findByRole('option', { name: 'trureturing / Revision 3' });
    expect(screen.getByRole('combobox', { name: 'workspace profile source' })).toHaveValue(
      'sandbox',
    );
    view.rerender(<ProfilePicker {...props} references={[]} />);
    await screen.findByRole('option', { name: 'trureturing / Revision 3' });
    expect(screen.getByRole('combobox', { name: 'workspace profile source' })).toHaveValue('own');
    expect(test.client.listProfiles).toHaveBeenCalledWith('workspace', 'own', expect.anything());
  });
});
