import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { WarmPools } from './WarmPools';
import { Operations } from './Operations';
import { operatorClient, warmPool } from './testFixtures';

it('identifies the pool and current target before disabling, with a no-write cancel path', async () => {
  const client = operatorClient();
  const user = userEvent.setup();
  render(<WarmPools client={client} />);
  await user.click(await screen.findByRole('button', { name: 'Disable pool' }));
  expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
  expect(screen.getByText(/Disable pool pool-1/)).toHaveTextContent('from 3 to 0');
  expect(screen.getByText(/Disable pool pool-1/)).toHaveTextContent('1000 millicores / 4096 MiB');
  await user.click(screen.getByRole('button', { name: 'Keep it' }));
  expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Disable pool' }));
  await user.dblClick(screen.getByRole('button', { name: 'Confirm disable' }));
  await screen.findByText('Target saved.');
  expect(client.admin.resizeWarmPool).toHaveBeenCalledExactlyOnceWith(warmPool.id, {
    expected_revision: warmPool.revision,
    target_replicas: 0,
  });
});

it('holds confirmation after the recorded pool revision changes', async () => {
  const client = operatorClient();
  const user = userEvent.setup();
  render(<WarmPools client={client} />);
  await user.click(await screen.findByRole('button', { name: 'Disable pool' }));
  const current = await client.admin.listWarmPools();
  client.admin.listWarmPools.mockResolvedValue({
    ...current,
    items: [{ ...warmPool, revision: 2, target_replicas: 5 }],
  });
  await user.click(screen.getByRole('button', { name: 'Refresh pools' }));
  await screen.findByText(/Your draft is retained/);
  expect(screen.getByRole('button', { name: 'Confirm disable' })).toBeDisabled();
  expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
});

it('removes pending confirmation when configuration authority is revoked', async () => {
  const client = operatorClient();
  const user = userEvent.setup();
  const view = render(<Operations client={client} capabilities={['configure_platform']} />);
  await user.click(await screen.findByRole('button', { name: 'Disable pool' }));
  expect(screen.getByRole('button', { name: 'Confirm disable' })).toBeEnabled();
  view.rerender(<Operations client={client} capabilities={[]} />);
  expect(screen.queryByRole('button', { name: 'Confirm disable' })).not.toBeInTheDocument();
  expect(client.admin.resizeWarmPool).not.toHaveBeenCalled();
});
