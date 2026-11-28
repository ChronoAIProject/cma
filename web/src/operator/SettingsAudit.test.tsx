import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { PlatformSettings } from './PlatformSettings';
import { operatorClient, stored } from './testFixtures';

const change = {
  actor_reference: 'actor-example',
  actor_name: 'Test administrator',
  at: '2026-02-03T00:00:00Z',
  revision: 3,
};

it('lists all committed settings before editing and shows attribution on the changed card only', async () => {
  const client = operatorClient();
  client.admin.readSettings.mockResolvedValue({
    ...stored,
    setting_changes: { agent_log_retention: change },
  });
  render(<PlatformSettings client={client} canReadAudit />);
  const overview = await screen.findByRole('navigation', { name: 'Settings overview' });
  expect(within(overview).getAllByRole('link')).toHaveLength(19);
  const card = screen.getByRole('region', { name: 'Agent log retention' });
  expect(within(card).getByText(/Last changed by Test administrator/)).toBeVisible();
  expect(card.querySelector('time')).toHaveAttribute('datetime', change.at);
  expect(
    within(screen.getByRole('region', { name: 'Visitor Sandboxes' })).getByText(
      'Last change not recorded',
    ),
  ).toBeVisible();
  expect(within(card).queryByRole('spinbutton')).toBeNull();
  await userEvent.click(within(card).getByRole('button', { name: 'Edit Agent log retention' }));
  const input = within(card).getByRole('spinbutton');
  await userEvent.clear(input);
  await userEvent.type(input, '42');
  expect(within(overview).getByText('30 days')).toBeVisible();
  expect(within(card).getByText('30 days')).toBeVisible();
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
  expect(client.admin.listActivity).not.toHaveBeenCalled();
});

it('withholds attribution without the activity capability and shows read-only pins', async () => {
  const client = operatorClient();
  client.admin.readSettings.mockResolvedValue({
    ...stored,
    setting_changes: { sandbox_profile: change },
  });
  render(<PlatformSettings client={client} />);
  const card = await screen.findByRole('region', { name: 'Default Sandbox Profile' });
  expect(within(card).getByText('sbp_default · revision 1')).toBeVisible();
  expect(within(card).getByText('Last change requires activity-log permission.')).toBeVisible();
  expect(within(card).queryByRole('button')).toBeNull();
  expect(screen.queryByText(/Test administrator/)).toBeNull();
});
