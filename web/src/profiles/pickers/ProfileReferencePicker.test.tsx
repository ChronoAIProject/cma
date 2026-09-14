import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import type { ProfileReference } from '../../api/types';
import { AGENT_PROFILE, MARKETPLACE_SANDBOX, marketplaceClient } from '../../test/profiles';
import { ProfileReferencePicker } from './ProfileReferencePicker';

it('selects pinned public revisions and enforces the ordered reference limit', async () => {
  const actor = userEvent.setup();
  const profiles = {
    listProfiles: vi
      .fn()
      .mockResolvedValue({ items: [{ ...AGENT_PROFILE, latestRevision: undefined }] }),
  };
  const listMarketplace = vi.fn().mockResolvedValue({
    items: [
      {
        ...MARKETPLACE_SANDBOX.summary,
        kind: 'agent',
        id: 'public-agent',
        name: 'Public research',
        latestRevision: 3,
      },
      {
        ...MARKETPLACE_SANDBOX.summary,
        kind: 'agent',
        id: 'second-agent',
        name: 'Second research',
        latestRevision: 2,
      },
    ],
  });
  const marketplace = marketplaceClient({ listMarketplace });
  const changed = vi.fn();
  function Picker() {
    const [value, setValue] = useState<readonly ProfileReference[]>([]);
    return (
      <ProfileReferencePicker
        kind="agent"
        profiles={profiles}
        marketplace={marketplace}
        value={value}
        onChange={(next) => {
          changed(next);
          setValue(next);
        }}
        maximum={1}
        disabled={false}
      />
    );
  }
  render(<Picker />);
  await screen.findByText('No matching published profiles');
  expect(screen.queryByRole('button', { name: 'Add Research agent' })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('radio', { name: 'Public profiles' }));
  fireEvent.change(screen.getByLabelText('Search agent profiles'), {
    target: { value: 'research' },
  });
  await actor.click(await screen.findByRole('button', { name: 'Add Public research' }));
  expect(changed).toHaveBeenLastCalledWith([{ id: 'public-agent', revision: 3 }]);
  expect(listMarketplace).toHaveBeenLastCalledWith('agent', 'research', expect.any(Object));
  expect(screen.getByRole('button', { name: 'Add Second research' })).toBeDisabled();
  await actor.click(screen.getByRole('button', { name: 'Remove agent profile 1' }));
  expect(screen.getByRole('button', { name: 'Add Second research' })).toBeEnabled();
});
