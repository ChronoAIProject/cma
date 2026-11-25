import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { PlatformSettings } from './PlatformSettings';
import { operatorClient } from './testFixtures';

it.each([
  'Agent log retention (days)',
  'Sandboxes per user',
  'Visitor Sandboxes',
  'Visitor launches per day',
])('exposes native validation for %s and clears it when corrected', async (label) => {
  const client = operatorClient();
  render(<PlatformSettings client={client} />);
  const input = (await screen.findByLabelText(label)) as HTMLInputElement;
  const original = input.value;
  for (const value of ['-1', '', '1.5', '4294967296']) {
    fireEvent.change(input, { target: { value } });
    expect(input.validity.valid).toBe(false);
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription(input.validationMessage);
    await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));
    expect(client.admin.replaceSettings).not.toHaveBeenCalled();
  }
  fireEvent.change(input, { target: { value: original } });
  expect(input).toHaveAttribute('aria-invalid', 'false');
  expect(input).not.toHaveAttribute('aria-describedby');
  await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await screen.findByText('Settings saved at revision 5.');
});
