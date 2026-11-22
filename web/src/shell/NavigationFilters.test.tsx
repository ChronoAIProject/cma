import { useState } from 'react';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { NavigationFilters } from './NavigationFilters';
import { expandNavigationFilters } from '../test/select';

function Filters() {
  const [status, setStatus] = useState('all');
  const [open, setOpen] = useState(false);
  return (
    <NavigationFilters
      view="sandbox"
      onView={() => {}}
      status={status}
      onStatus={setStatus}
      open={open}
      onOpenChange={setOpen}
    />
  );
}

it('offers distinct startup states and names the selected state in the summary', async () => {
  render(<Filters />);
  const user = userEvent.setup();
  await expandNavigationFilters(user);
  for (const label of ['Provisioning', 'Installing', 'Updating runtime', 'Updating access']) {
    const picker = screen.getByRole('button', { name: 'Sandbox status filter' });
    await user.click(picker);
    const menu = within(screen.getByRole('menu', { name: 'Sandbox status filter' }));
    expect(menu.getAllByRole('menuitemradio', { name: 'Provisioning' })).toHaveLength(1);
    expect(menu.getAllByRole('menuitemradio', { name: 'Installing' })).toHaveLength(1);
    expect(menu.getAllByRole('menuitemradio', { name: 'Updating runtime' })).toHaveLength(1);
    expect(menu.getAllByRole('menuitemradio', { name: 'Updating access' })).toHaveLength(1);
    expect(menu.queryByRole('menuitemradio', { name: 'Starting' })).not.toBeInTheDocument();
    await user.click(menu.getByRole('menuitemradio', { name: label }));
    expect(picker).toHaveTextContent(label);
    expect(screen.getByRole('button', { name: 'Filter' })).toHaveAccessibleDescription(label);
  }
});
