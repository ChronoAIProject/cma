import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { Operations } from './Operations';
import { operatorClient } from './testFixtures';
import { metricRow } from '../usage/metrics/testFixtures';

it.each(['Alex Chen', 'Morgan Park', null])(
  'identifies owner %s in retirement confirmation without exposing subject IDs',
  async (display_name) => {
    const client = operatorClient();
    client.adminUsageMetrics.sandboxes.mockResolvedValue({
      report_id: 'report-one',
      as_of: 1,
      items: [{ ...metricRow, owner: { subject: 'private-owner', display_name } }],
      next: null,
      total_count: 1,
    });
    const user = userEvent.setup();
    render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'control_any_sandbox']} />,
    );
    await user.click(await screen.findByRole('button', { name: 'Retire' }));
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveTextContent(`Owner: ${display_name ?? 'Name unavailable'}`);
    expect(dialog).toHaveTextContent(metricRow.sandbox_id);
    expect(dialog).not.toHaveTextContent('private-owner');
    expect(within(dialog).getByRole('button', { name: 'Retire Sandbox' })).toBeDisabled();
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(client.admin.retireSandbox).not.toHaveBeenCalled();
  },
);
