import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { BridgePartitions } from './BridgePartitions';
import { allocationId, partitionCommandFixture, result } from './partition-commands.test-support';
async function inspect() {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Show partitions' }));
  await user.click(await screen.findByRole('button', { name: 'Inspect generation 1' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Reset partition generation' })).toBeEnabled(),
  );
  return user;
}
async function confirm(action: 'reset' | 'detach', user: ReturnType<typeof userEvent.setup>) {
  await user.click(
    screen.getByRole('button', {
      name: action === 'reset' ? 'Reset partition generation' : 'Detach partition',
    }),
  );
  const dialog = within(screen.getByRole('dialog'));
  const confirm = dialog.getByRole('button', {
    name: action === 'reset' ? 'Confirm generation reset' : 'Confirm detach',
  });
  expect(confirm).toBeDisabled();
  await user.click(dialog.getByRole('checkbox'));
  await user.click(confirm);
}

it('requires deliberate exact-generation confirmation and preserves the native conversation when resetting', async () => {
  const f = partitionCommandFixture();
  render(<BridgePartitions {...f.props} />);
  const user = await inspect();
  await user.click(screen.getByRole('button', { name: 'Reset partition generation' }));
  expect(screen.getByRole('dialog')).toHaveTextContent('Agent agt_exact, generation 1.');
  expect(screen.getByRole('dialog')).toHaveTextContent(
    'Its existing conversation and context continue unchanged.',
  );
  expect(f.writes).toHaveLength(0);
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
  expect(f.writes).toHaveLength(0);
  await confirm('reset', user);
  await screen.findByText(/Reset accepted. Generation 2 is ready/);
  expect(f.writes).toHaveLength(1);
  expect(f.writes[0]).toMatchObject({
    path: `/api/v1/bridges/brg_one/partitions/${allocationId}/reset`,
    body: { expected_bridge_revision: 5, expected_revision: 3 },
  });
  expect(f.writes[0]!.key).toBeTruthy();
});

it('retains the original detach request after a lost reply across access and revision refresh', async () => {
  const f = partitionCommandFixture();
  f.state.loseReply = true;
  const view = render(<BridgePartitions {...f.props} />);
  const user = await inspect();
  await confirm('detach', user);
  await screen.findByText(/Partition command acceptance could not be confirmed/);
  expect(f.writes).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Inspect generation 1' })).toBeDisabled();
  view.rerender(
    <BridgePartitions {...f.props} allowed={false} revision={undefined} ownerScope={undefined} />,
  );
  expect(
    screen.queryByRole('button', { name: 'Recover original partition request' }),
  ).not.toBeInTheDocument();
  view.rerender(<BridgePartitions {...f.props} revision={99} />);
  const recover = await screen.findByRole('button', { name: 'Recover original partition request' });
  await waitFor(() => expect(recover).toBeEnabled());
  await user.click(recover);
  await screen.findByText(
    'Partition generation 1 detached. The Agent conversation and history are retained.',
  );
  expect(f.writes).toHaveLength(2);
  expect(f.writes[1]).toEqual(f.writes[0]);
});

it('keeps cancellation available when the reviewed revision changes and never submits stale intent', async () => {
  const f = partitionCommandFixture();
  const view = render(<BridgePartitions {...f.props} />);
  const user = await inspect();
  await user.click(screen.getByRole('button', { name: 'Detach partition' }));
  await user.click(within(screen.getByRole('dialog')).getByRole('checkbox'));
  view.rerender(<BridgePartitions {...f.props} revision={6} />);
  expect(screen.getByRole('button', { name: 'Confirm detach' })).toBeDisabled();
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
  expect(f.writes).toHaveLength(0);
});

it('discards a late accepted command after switching viewer identity', async () => {
  const f = partitionCommandFixture();
  let release!: (response: Response) => void;
  f.state.pause = new Promise((resolve) => {
    release = resolve;
  });
  const view = render(<BridgePartitions {...f.props} />);
  const user = await inspect();
  await confirm('reset', user);
  await screen.findByText('Checking acceptance of the original partition request');
  view.rerender(<BridgePartitions {...f.props} viewerId="another" allowed={false} />);
  await act(async () => release(Response.json(result(), { status: 202 })));
  expect(screen.queryByText(/Reset accepted/)).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Recover original partition request' }),
  ).not.toBeInTheDocument();
  expect(f.writes).toHaveLength(1);
});
