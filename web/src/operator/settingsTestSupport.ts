import { fireEvent, screen } from '@testing-library/react';

export async function openSettingsEditors() {
  await screen.findByRole('navigation', { name: 'Settings overview' });
  for (const button of screen.getAllByRole('button', { name: /^(Edit |View fields for )/ })) {
    fireEvent.click(button);
  }
}
