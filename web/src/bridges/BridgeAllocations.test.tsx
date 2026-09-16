import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { BridgeAllocations } from './BridgeAllocations';
import { cleanupFixture } from './cleanup.test-support';

async function inspect() {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Show allocations' }));
  await user.click(await screen.findByRole('button', { name: 'Inspect cleanup for generation 1' }));
  await screen.findByRole('heading', { name: 'Cleanup not requested' });
  return user;
}
async function consent() {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Remove retired child' }));
  const modal = within(screen.getByRole('dialog'));
  expect(modal.getByRole('button', { name: 'Request child removal' })).toBeDisabled();
  await user.click(modal.getByRole('checkbox'));
  await user.click(modal.getByRole('button', { name: 'Request child removal' }));
}

it('browses empty continued pages using the exact opaque cursor and links the selected target', async () => {
  const test = cleanupFixture();
  test.state.emptyFirst = true;
  render(<BridgeAllocations {...test.props} />);
  expect(test.fetch).not.toHaveBeenCalled();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Show allocations' }));
  await screen.findByText('No accessible allocations on this page. Continue to the next page.');
  await user.click(screen.getByRole('button', { name: 'Next allocations' }));
  await screen.findByRole('button', { name: 'Inspect cleanup for generation 1' });
  expect(new URL(String(test.fetch.mock.calls[1]![0])).searchParams.get('cursor')).toBe(
    'opaque/+/v1?part=1',
  );
  expect(
    screen.getByRole('link', { name: `Agent ${test.allocation.target.agent_id}` }),
  ).toHaveAttribute('href', `/agents/${test.allocation.target.agent_id}`);
  expect(
    screen.getByRole('link', { name: `Sandbox ${test.allocation.target.sandbox_id}` }),
  ).toHaveAttribute('href', `/sandboxes/${test.allocation.target.sandbox_id}`);
  await user.click(screen.getByRole('button', { name: 'Previous allocations' }));
  await screen.findByText('No accessible allocations on this page. Continue to the next page.');
  expect(test.writes).toHaveLength(0);
});

it('requires explicit destructive consent and distinguishes acceptance, holds and complete removal', async () => {
  const test = cleanupFixture();
  render(<BridgeAllocations {...test.props} />);
  const user = await inspect();
  await user.click(screen.getByRole('button', { name: 'Remove retired child' }));
  expect(screen.getByRole('dialog')).toHaveTextContent(
    'The Workspace, sibling Agents and retained history stay in place.',
  );
  expect(test.writes).toHaveLength(0);
  await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }));
  expect(test.writes).toHaveLength(0);
  await consent();
  await screen.findByRole('heading', { name: 'Cleanup requested' });
  expect(screen.getByText('An accepted command is still pending.')).toBeInTheDocument();
  expect(screen.getByText(/The Agent reservation has not been released/)).toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
  test.state.accepted = {
    ...test.state.accepted!,
    progress: 'complete',
    holds: [],
    agent_capacity_released: true,
  };
  await user.click(screen.getByRole('button', { name: 'Refresh cleanup status' }));
  await screen.findByRole('heading', { name: 'Child cleanup complete' });
  expect(screen.getByText('The Agent reservation has been released.')).toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
});

it.each(['loseReply', 'conflict'] as const)(
  'retains original revisions and key after %s through inventory and Bridge access refresh',
  async (failure) => {
    const test = cleanupFixture();
    test.state[failure] = true;
    const view = render(<BridgeAllocations {...test.props} />);
    const user = await inspect();
    await consent();
    await screen.findByText(/Cleanup acceptance could not be confirmed/);
    expect(test.writes).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: 'Refresh allocations' }));
    await screen.findByRole('button', { name: 'Inspect cleanup for generation 1' });
    view.rerender(<BridgeAllocations {...test.props} allowed={false} revision={undefined} />);
    expect(
      screen.queryByRole('button', { name: 'Recover original cleanup request' }),
    ).not.toBeInTheDocument();
    view.rerender(<BridgeAllocations {...test.props} revision={99} />);
    const recover = await screen.findByRole('button', { name: 'Recover original cleanup request' });
    await waitFor(() => expect(recover).toBeEnabled());
    expect(test.writes).toHaveLength(1);
    await user.click(recover);
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: 'Recover original cleanup request' }),
      ).not.toBeInTheDocument(),
    );
    expect(test.writes).toHaveLength(2);
    expect(test.writes[1]).toEqual(test.writes[0]);
    expect(screen.queryByText('private server detail')).not.toBeInTheDocument();
  },
);

it('keeps Cancel available when the reviewed Bridge revision changes and never submits stale consent', async () => {
  const test = cleanupFixture();
  const view = render(<BridgeAllocations {...test.props} />);
  const user = await inspect();
  await user.click(screen.getByRole('button', { name: 'Remove retired child' }));
  await user.click(within(screen.getByRole('dialog')).getByRole('checkbox'));
  view.rerender(<BridgeAllocations {...test.props} revision={99} />);
  const modal = within(screen.getByRole('dialog'));
  expect(modal.getByRole('button', { name: 'Request child removal' })).toBeDisabled();
  expect(modal.getByRole('button', { name: 'Cancel' })).toBeEnabled();
  await user.click(modal.getByRole('button', { name: 'Cancel' }));
  expect(test.writes).toHaveLength(0);
});

it('aborts and clears pending cleanup when the current account changes, ignoring a late response', async () => {
  const test = cleanupFixture();
  let release!: () => void;
  test.state.hold = new Promise<void>((resolve) => {
    release = resolve;
  });
  const view = render(<BridgeAllocations {...test.props} />);
  await inspect();
  await consent();
  await waitFor(() => expect(test.writes).toHaveLength(1));
  view.rerender(<BridgeAllocations {...test.props} viewerId="another-manager" />);
  expect(test.state.signal?.aborted).toBe(true);
  await act(async () => release());
  expect(screen.queryByText(/Cleanup requested/)).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Recover original cleanup request' }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Show allocations' })).toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
});

it('withholds private reads from ordinary viewers and refuses a changed target or access denial', async () => {
  const test = cleanupFixture();
  const view = render(<BridgeAllocations {...test.props} allowed={false} />);
  await act(async () => {});
  expect(test.fetch).not.toHaveBeenCalled();
  view.rerender(<BridgeAllocations {...test.props} />);
  const user = await inspect();
  test.state.corruptTarget = true;
  await user.click(screen.getByRole('button', { name: 'Refresh cleanup status' }));
  await screen.findByRole('alert');
  expect(screen.queryByRole('button', { name: 'Remove retired child' })).not.toBeInTheDocument();
  test.state.denied = true;
  await user.click(screen.getByRole('button', { name: 'Refresh allocations' }));
  await screen.findByText(
    'Allocation access could not be checked. Refresh to check current access.',
  );
  expect(
    screen.queryByRole('link', { name: `Agent ${test.allocation.target.agent_id}` }),
  ).not.toBeInTheDocument();
  expect(test.writes).toHaveLength(0);
});

it.each([
  { placement: 'new_sandbox', state: 'retired' },
  { placement: 'reuse_parent', state: 'ready' },
] as const)(
  'offers no cleanup command for unsupported placement/state $placement/$state',
  async (context) => {
    const test = cleanupFixture(context);
    render(<BridgeAllocations {...test.props} />);
    await inspect();
    expect(screen.queryByRole('button', { name: 'Remove retired child' })).not.toBeInTheDocument();
    expect(
      screen.getByText(
        'Only a retired child created in a shared parent supports this cleanup action.',
      ),
    ).toBeInTheDocument();
    expect(test.writes).toHaveLength(0);
  },
);
