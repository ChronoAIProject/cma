import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { BridgeAllocations } from './BridgeAllocations';
import { workspaceCleanupFixture } from './workspace-cleanup.test-support';

async function inspectChild() {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Show allocations' }));
  await user.click(await screen.findByRole('button', { name: 'Inspect cleanup for generation 1' }));
  return user;
}
async function inspectWorkspace() {
  const user = await inspectChild();
  await user.click(await screen.findByRole('button', { name: 'Inspect Workspace cleanup' }));
  await screen.findByRole('heading', { name: 'Workspace cleanup not requested' });
  return user;
}
async function consent() {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Remove owned Workspace' }));
  const modal = within(screen.getByRole('dialog'));
  expect(modal.getByRole('button', { name: 'Request removal' })).toBeDisabled();
  await user.click(modal.getByRole('checkbox'));
  await user.click(modal.getByRole('button', { name: 'Request removal' }));
}

it('requires completed child proof before inspecting and separate explicit consent before any Workspace POST', async () => {
  const test = workspaceCleanupFixture();
  test.state.child.progress = 'requested';
  test.state.child.agent_capacity_released = false;
  render(<BridgeAllocations {...test.props} />);
  const user = await inspectChild();
  const inspect = await screen.findByRole('button', { name: 'Inspect Workspace cleanup' });
  expect(inspect).toBeDisabled();
  expect(test.fetch.mock.calls.some(([url]) => String(url).includes('/workspace-cleanup'))).toBe(
    false,
  );
  test.state.child.progress = 'complete';
  test.state.child.agent_capacity_released = true;
  await user.click(screen.getByRole('button', { name: 'Refresh cleanup status' }));
  await waitFor(() => expect(inspect).toBeEnabled());
  await user.click(inspect);
  await screen.findByRole('heading', { name: 'Workspace cleanup not requested' });
  await user.click(screen.getByRole('button', { name: 'Remove owned Workspace' }));
  const modal = within(screen.getByRole('dialog'));
  expect(modal.getByText(new RegExp(test.allocation.target.workspace_id))).toBeInTheDocument();
  expect(screen.getByRole('dialog')).toHaveTextContent(
    'The Sandbox, sibling Workspaces and retained history stay in place.',
  );
  await user.click(modal.getByRole('button', { name: 'Cancel' }));
  expect(test.writes).toHaveLength(0);
  await consent();
  await screen.findByRole('heading', { name: 'Workspace cleanup requested' });
  expect(test.writes[0]!.body).toEqual(test.original);
  expect(screen.getByText(/The Workspace reservation has not been released/)).toBeInTheDocument();
  test.state.accepted = {
    ...test.state.accepted!,
    progress: 'complete',
    holds: [],
    workspace_capacity_released: true,
  };
  test.state.workspace.workspace_revision = 8;
  await user.click(screen.getByRole('button', { name: 'Refresh Workspace cleanup' }));
  await screen.findByRole('heading', { name: 'Workspace cleanup complete' });
  expect(
    screen.getByText('The owned Workspace reservation has been released.'),
  ).toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
});

it.each(['lost', 'conflict', 'unavailable'] as const)(
  'retains all original consent fields and key through %s, paging and access refresh despite new revision or capability',
  async (failure) => {
    const test = workspaceCleanupFixture();
    test.state.loseReply = failure === 'lost';
    test.state.failure = failure === 'conflict' ? 409 : failure === 'unavailable' ? 503 : 0;
    const view = render(<BridgeAllocations {...test.props} />);
    const user = await inspectWorkspace();
    await consent();
    await screen.findByText(/Workspace cleanup acceptance could not be confirmed/);
    expect(
      screen.queryByRole('button', { name: 'Recover original cleanup request' }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Inspect cleanup for generation 1' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Refresh allocations' }));
    view.rerender(<BridgeAllocations {...test.props} allowed={false} revision={undefined} />);
    expect(
      screen.queryByRole('region', { name: 'Owned Workspace cleanup' }),
    ).not.toBeInTheDocument();
    test.state.workspace.runtime_supported = false;
    test.state.workspace.workspace_revision = 9;
    view.rerender(<BridgeAllocations {...test.props} revision={99} />);
    const recover = await screen.findByRole('button', {
      name: 'Recover original Workspace cleanup request',
    });
    await waitFor(() => expect(recover).toBeEnabled());
    expect(test.writes).toHaveLength(1);
    await user.click(recover);
    await waitFor(() =>
      expect(
        screen.queryByRole('button', { name: 'Recover original Workspace cleanup request' }),
      ).not.toBeInTheDocument(),
    );
    expect(test.writes).toHaveLength(2);
    expect(test.writes[1]).toEqual(test.writes[0]);
    expect(test.writes[1]!.body).toEqual(test.original);
    expect(screen.queryByText('private server detail')).not.toBeInTheDocument();
  },
);

it.each(['bridge', 'workspace', 'capability', 'child_receipt'] as const)(
  'disables reviewed consent after current %s changes while Cancel remains available',
  async (change) => {
    const test = workspaceCleanupFixture();
    const view = render(<BridgeAllocations {...test.props} />);
    const user = await inspectWorkspace();
    await user.click(screen.getByRole('button', { name: 'Remove owned Workspace' }));
    await user.click(within(screen.getByRole('dialog')).getByRole('checkbox'));
    if (change === 'bridge') view.rerender(<BridgeAllocations {...test.props} revision={99} />);
    if (change === 'workspace') test.state.workspace.workspace_revision = 9;
    if (change === 'capability') {
      test.state.workspace.runtime_supported = false;
      test.state.workspace.eligible = false;
      test.state.workspace.holds = ['runtime_unsupported'];
    }
    if (change === 'child_receipt')
      test.state.child.receipt = { ...test.state.child.receipt!, id: `bclj_${'8'.repeat(64)}` };
    if (change !== 'bridge')
      await act(async () => {
        window.dispatchEvent(new Event('focus'));
      });
    const modal = within(screen.getByRole('dialog'));
    await waitFor(() =>
      expect(modal.getByRole('button', { name: 'Request removal' })).toBeDisabled(),
    );
    expect(modal.getByRole('button', { name: 'Cancel' })).toBeEnabled();
    await user.click(modal.getByRole('button', { name: 'Cancel' }));
    expect(test.writes).toHaveLength(0);
  },
);

it.each(['viewer', 'owner', 'client'] as const)(
  'aborts old observation and clears uncertain Workspace consent when %s identity changes',
  async (change) => {
    const test = workspaceCleanupFixture();
    let release!: () => void;
    test.state.hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    const view = render(<BridgeAllocations {...test.props} />);
    await inspectWorkspace();
    await consent();
    await waitFor(() => expect(test.writes).toHaveLength(1));
    const props =
      change === 'viewer'
        ? { viewerId: 'another' }
        : change === 'owner'
          ? { ownerScope: 'organization:another' }
          : { client: workspaceCleanupFixture().props.client };
    view.rerender(<BridgeAllocations {...test.props} {...props} />);
    expect(test.state.signal?.aborted).toBe(true);
    await act(async () => release());
    expect(
      screen.queryByRole('button', { name: 'Recover original Workspace cleanup request' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('heading', { name: 'Workspace cleanup requested' }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Show allocations' })).toBeInTheDocument();
    expect(test.writes).toHaveLength(1);
  },
);

it('explains unsupported runtime and refuses absent capability, a mismatched target and lost authority', async () => {
  const test = workspaceCleanupFixture();
  test.state.workspace.runtime_supported = false;
  test.state.workspace.eligible = false;
  test.state.workspace.holds = ['runtime_unsupported'];
  const view = render(<BridgeAllocations {...test.props} allowed={false} />);
  expect(test.fetch).not.toHaveBeenCalled();
  view.rerender(<BridgeAllocations {...test.props} />);
  const user = await inspectWorkspace();
  expect(screen.getByRole('button', { name: 'Remove owned Workspace' })).toBeDisabled();
  expect(
    screen.getByText('This Sandbox does not currently support safe owned Workspace cleanup.'),
  ).toBeInTheDocument();
  test.state.corruptTarget = true;
  await user.click(screen.getByRole('button', { name: 'Refresh Workspace cleanup' }));
  await screen.findByRole('alert');
  expect(screen.getByRole('button', { name: 'Remove owned Workspace' })).toBeDisabled();
  test.state.denied = true;
  await act(async () => {
    window.dispatchEvent(new Event('focus'));
  });
  expect(
    screen.queryByRole('link', { name: `Workspace ${test.allocation.target.workspace_id}` }),
  ).not.toBeInTheDocument();
  expect(test.writes).toHaveLength(0);
});
