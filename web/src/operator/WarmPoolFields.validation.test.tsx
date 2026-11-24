import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { WarmPools } from './WarmPools';
import { operatorClient } from './testFixtures';

it.each([
  ['CPU (millicores)', '0'],
  ['Memory (MiB)', '0'],
  ['Ready Sandbox target', '-1'],
])('explains invalid %s before saving a pool', async (label, invalid) => {
  const client = operatorClient();
  const user = userEvent.setup();
  render(<WarmPools client={client} />);
  await user.click(await screen.findByText('Configure a warm pool'));
  const form = screen.getByRole('button', { name: 'Save pool target' }).closest('form')!;
  const input = within(form).getByLabelText(label) as HTMLInputElement;
  const original = input.value;
  for (const value of [invalid, '', '1.5', '1048577']) {
    fireEvent.change(input, { target: { value } });
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleName(label);
    expect(input).toHaveAccessibleDescription(input.validationMessage);
    expect(screen.getByRole('button', { name: 'Save pool target' })).toBeDisabled();
    fireEvent.submit(form);
    expect(client.admin.configureWarmPool).not.toHaveBeenCalled();
  }
  fireEvent.change(input, { target: { value: original } });
  expect(input).toHaveAttribute('aria-invalid', 'false');
  expect(input).not.toHaveAttribute('aria-describedby');
  await user.click(screen.getByRole('button', { name: 'Save pool target' }));
  await screen.findByText(/Pool target saved/);
});

it('explains invalid existing-pool targets without sending a resize', async () => {
  const client = operatorClient();
  render(<WarmPools client={client} />);
  const row = await screen.findByRole('row', { name: /Warm pool/ });
  const input = within(row).getByLabelText('Ready Sandbox target') as HTMLInputElement;
  fireEvent.change(input, { target: { value: '-1' } });
  expect(input).toHaveAttribute('aria-invalid', 'true');
  expect(input).toHaveAccessibleDescription(input.validationMessage);
  fireEvent.submit(input.closest('form')!);
  expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
  fireEvent.change(input, { target: { value: '4' } });
  expect(input).toHaveAttribute('aria-invalid', 'false');
  await userEvent.click(within(row).getByRole('button', { name: 'Update target' }));
  await screen.findByText('Target saved.');
});
