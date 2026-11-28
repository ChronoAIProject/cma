import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { WarmPools } from './WarmPools';
import { operatorClient, warmPool } from './testFixtures';
import { cell } from './warmPoolTestHelpers';

describe('warm pool tables', () => {
  it('names each column, draws the status badge and edits the target in its own cell', async () => {
    render(<WarmPools client={operatorClient()} />);
    const row = await screen.findByRole('row', { name: 'Warm pool 1000 millicores / 4096 MiB' });
    const table = screen.getByRole('table', { name: 'Current runtime pools' });
    expect(
      within(table)
        .getAllByRole('columnheader')
        .map((column) => column.textContent),
    ).toEqual([
      'Size',
      'Status',
      'Desired',
      'Ready',
      'Provider target',
      'Pending',
      'Target',
      'Actions',
    ]);
    expect(within(cell(row, 'Status')).getByText('Pending').closest('.badge')).toHaveAttribute(
      'data-tone',
      'active',
    );
    expect(within(cell(row, 'Target')).getByLabelText('Ready Sandbox target')).toHaveValue(3);
    expect(within(row).getByRole('button', { name: 'Disable pool' })).toBeEnabled();
    // A settled, observed pool has nothing to say beneath its row.
    expect(table.querySelector('.table__note')).toBeNull();
    expect(screen.getAllByText(/Set a target to 0 to disable spare capacity/)).toHaveLength(1);
  });

  it('says what the controller has not confirmed in the pool’s note row', async () => {
    const client = operatorClient();
    client.admin.listWarmPools.mockResolvedValue({
      items: [{ ...warmPool, observed: false, ready_replicas: null, status: 'unavailable' }],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_image: warmPool.image,
      available: true,
    });
    render(<WarmPools client={client} />);
    const row = await screen.findByRole('row', { name: /Warm pool/ });
    expect(within(cell(row, 'Status')).getByText('Unavailable').closest('.badge')).toHaveAttribute(
      'data-tone',
      'warning',
    );
    const note = row.nextElementSibling!;
    expect(note).toHaveClass('table__note');
    expect(note).toHaveTextContent('Waiting for the controller to confirm current capacity.');
  });

  it('says an empty current runtime has no pools in the table, keeping its headers', async () => {
    const client = operatorClient();
    client.admin.listWarmPools.mockResolvedValue({
      items: [],
      default_cpu_millis: 1000,
      default_memory_mib: 1024,
      default_image: warmPool.image,
      available: true,
    });
    render(<WarmPools client={client} />);
    expect(
      await screen.findByRole('cell', { name: /No warm pools configured for the current runtime/ }),
    ).toBeVisible();
    expect(screen.getAllByRole('columnheader')).toHaveLength(8);
  });
});
