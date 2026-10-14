import { chooseOptionValue } from '../test/select';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { Operations, canOpenOperations } from './Operations';
import { detail, now, operatorClient, profileRow, sandboxRow } from './testFixtures';

describe('operator capabilities and record views', () => {
  it("shows people by name and keeps another owner's Agents under the right Workspace", async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.readSandbox.mockResolvedValue({
      ...detail,
      workspaces: [
        ...detail.workspaces,
        { ...detail.workspaces[0]!, id: 'wks-2', name: 'Other Workspace' },
      ],
      agents: [
        ...detail.agents,
        { ...detail.agents[0]!, id: 'agt-2', workspace_id: 'wks-2', title: 'Review assistant' },
      ],
    });
    render(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    expect(await screen.findByText('Owner Alex Chen')).toBeInTheDocument();
    expect(screen.queryByText('owner-1')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Details for Research Sandbox' }));
    const drawer = screen.getByRole('dialog');
    const workspace = (await within(drawer).findByText('Research Workspace: ready')).closest('li')!;
    expect(within(workspace).getByText('Research assistant: ready')).toBeInTheDocument();
    expect(within(workspace).queryByText('Review assistant: ready')).not.toBeInTheDocument();
    expect(within(drawer).getByText('Alex Chen')).toBeInTheDocument();
    expect(within(drawer).queryByText('owner-1')).not.toBeInTheDocument();
    expect(client.admin.retireSandbox).not.toHaveBeenCalled();
  });

  it('uses explicit unavailable names for unknown Activity users without exposing their IDs', async () => {
    const client = operatorClient();
    client.admin.listActivity.mockResolvedValue({
      items: [
        {
          id: 'act-test',
          at: now,
          actor: 'unknown-person-id',
          actor_name: null,
          action: 'read',
          target: { kind: 'subject', id: 'unknown-target-id' },
          target_name: null,
          outcome: { status: 'succeeded' },
          detail: {},
        },
      ],
      next: null,
    });
    render(<Operations client={client} capabilities={['read_activity_log']} />);
    expect(await screen.findByText('Actor Name unavailable')).toBeInTheDocument();
    expect(screen.getByText('User Name unavailable')).toBeInTheDocument();
    expect(screen.queryByText('unknown-person-id')).not.toBeInTheDocument();
    expect(screen.queryByText('unknown-target-id')).not.toBeInTheDocument();
  });

  it('removes a destructive confirmation when control permission is revoked', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const { rerender } = render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'control_any_sandbox']} />,
    );
    await user.click(await screen.findByRole('button', { name: 'Retire' }));
    await user.click(screen.getByRole('checkbox'));
    rerender(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retire' })).toBeDisabled();
    expect(client.admin.retireSandbox).not.toHaveBeenCalled();
  });
  it('does not mount or fetch without an available view capability', () => {
    const client = operatorClient();
    const { container } = render(
      <Operations client={client} capabilities={['control_any_sandbox']} />,
    );
    expect(container).toBeEmptyDOMElement();
    expect(canOpenOperations([])).toBe(false);
    expect(canOpenOperations(['configure_platform'])).toBe(true);
    for (const method of Object.values(client.admin)) expect(method).not.toHaveBeenCalled();
    expect(client.listPublicProfiles).not.toHaveBeenCalled();
  });

  it('independently gates tabs and switches to an allowed view after revocation', async () => {
    const client = operatorClient();
    const { rerender } = render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'read_activity_log']} />,
    );
    await screen.findByText('Research Sandbox');
    expect(screen.getAllByRole('tab').map((tab) => tab.textContent)).toEqual([
      'Sandboxes',
      'Profiles',
      'Activity',
    ]);
    expect(screen.queryByRole('tab', { name: 'Platform settings' })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Sandboxes' }), { key: 'ArrowRight' });
    expect(screen.getByRole('tab', { name: 'Profiles' })).toHaveFocus();
    await screen.findByText('Research Agent');
    rerender(<Operations client={client} capabilities={['read_activity_log']} />);
    await screen.findByText('Actor Morgan Park');
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    expect(client.admin.readSettings).not.toHaveBeenCalled();
    expect(screen.queryByText('must not render')).not.toBeInTheDocument();
  });

  it('applies Sandbox filters explicitly and retains them and rows after a failed refresh', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    await screen.findByText('Research Sandbox');
    await user.type(screen.getByLabelText('Sandbox owner'), 'owner-2');
    await chooseOptionValue(user, screen.getByLabelText('Sandbox status'), 'failed');
    await user.type(screen.getByLabelText('Runtime version'), 'runtime-3');
    expect(client.admin.listSandboxes).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await waitFor(() =>
      expect(client.admin.listSandboxes).toHaveBeenLastCalledWith(
        expect.objectContaining({
          owner: 'owner-2',
          status: 'failed',
          runtime_version: 'runtime-3',
        }),
      ),
    );
    client.admin.listSandboxes.mockRejectedValueOnce(
      new ApiError(503, 'Unavailable', 'operator_service_unavailable', 'request-830'),
    );
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    await screen.findByText('operator_service_unavailable');
    expect(screen.getByText('Research Sandbox')).toBeInTheDocument();
    expect(screen.getByLabelText('Sandbox owner')).toHaveValue('owner-2');
    expect(screen.getByText('request-830')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retire' })).toBeDisabled();
  });

  it('opens a focus-trapped hierarchy and safe event timeline drawer then restores focus', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.sandboxEvents.mockResolvedValue([
      {
        sandbox_id: sandboxRow.id,
        seq: 1,
        appended_at: now,
        kind: { type: 'checkpoint_sealed', object_reference: 'internal-checkpoint-authority' },
      },
    ]);
    render(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    const trigger = await screen.findByRole('button', { name: 'Details for Research Sandbox' });
    await user.click(trigger);
    const drawer = await screen.findByRole('dialog', { name: 'Sandbox details' });
    await within(drawer).findByText('Research Workspace: ready');
    await within(drawer).findByText('Research assistant: ready');
    expect(within(drawer).getByText('checkpoint sealed')).toBeInTheDocument();
    expect(screen.queryByText('internal-checkpoint-authority')).not.toBeInTheDocument();
    expect(client.admin.sandboxEvents).toHaveBeenCalledWith(
      'sbx-1',
      expect.objectContaining({ after: 0, limit: 50 }),
    );
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('requires acknowledgement and suppresses repeated retirement while pending', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    let resolve!: (value: typeof detail.sandbox) => void;
    client.admin.retireSandbox.mockReturnValue(
      new Promise((done) => {
        resolve = done;
      }),
    );
    render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'control_any_sandbox']} />,
    );
    await user.click(await screen.findByRole('button', { name: 'Retire' }));
    const dialog = screen.getByRole('dialog');
    const confirm = within(dialog).getByRole('button', { name: 'Retire Sandbox' });
    expect(confirm).toBeDisabled();
    await user.click(within(dialog).getByRole('checkbox'));
    await user.dblClick(confirm);
    expect(client.admin.retireSandbox).toHaveBeenCalledTimes(1);
    expect(confirm).toBeDisabled();
    await act(async () => resolve({ ...detail.sandbox, status: 'retiring' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(client.admin.listSandboxes).toHaveBeenCalledTimes(2);
  });

  it('keeps an unknown retirement outcome and sends only a read on the next confirmation', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.retireSandbox.mockRejectedValue(new Error('Disconnected'));
    client.admin.readSandbox.mockResolvedValue({
      ...detail,
      sandbox: { ...detail.sandbox, status: 'retiring' },
    });
    render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'control_any_sandbox']} />,
    );
    await user.click(await screen.findByRole('button', { name: 'Retire' }));
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: 'Retire Sandbox' }));
    const check = await screen.findByRole('button', { name: 'Check current state' });
    expect(check).toBeDisabled();
    await user.click(screen.getByRole('checkbox'));
    await user.click(check);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(client.admin.retireSandbox).toHaveBeenCalledTimes(1);
    expect(client.admin.readSandbox).toHaveBeenCalledTimes(1);
  });

  it('filters profiles, marks protected defaults, and keeps typed refusal in confirmation', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.listProfiles.mockResolvedValue({
      items: [{ ...profileRow, id: 'agp_default', name: 'Default Agent' }, profileRow],
      next: null,
    });
    client.admin.retireProfile.mockRejectedValue(new ApiError(403, 'Refused', 'not_an_operator'));
    render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'control_any_sandbox']} />,
    );
    await user.click(screen.getByRole('tab', { name: 'Profiles' }));
    await screen.findByText('Default Agent');
    expect(screen.getAllByRole('button', { name: 'Retire' })[0]).toBeDisabled();
    expect(screen.getByText('Deployment-owned default.')).toBeInTheDocument();
    await chooseOptionValue(user, screen.getByLabelText('Profile kind'), 'agent');
    await chooseOptionValue(user, screen.getByLabelText('Profile visibility'), 'public');
    await user.type(screen.getByLabelText('Profile owner'), 'owner-1');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await waitFor(() =>
      expect(client.admin.listProfiles).toHaveBeenLastCalledWith(
        expect.objectContaining({ kind: 'agent', visibility: 'public', owner: 'owner-1' }),
      ),
    );
    await user.click(screen.getAllByRole('button', { name: 'Retire' })[1]!);
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: 'Retire profile' }));
    await screen.findByText(/required operator capability/);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(client.admin.retireProfile).toHaveBeenCalledWith('agent', 'agp-1');
  });

  it('offers a read after ambiguous profile retirement and leaves Cancel functional', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.retireProfile.mockRejectedValue(new Error('Disconnected'));
    render(
      <Operations client={client} capabilities={['read_all_sandboxes', 'control_any_sandbox']} />,
    );
    await user.click(screen.getByRole('tab', { name: 'Profiles' }));
    await user.click(await screen.findByRole('button', { name: 'Retire' }));
    await user.click(screen.getByRole('checkbox'));
    await user.click(screen.getByRole('button', { name: 'Retire profile' }));
    const check = await screen.findByRole('button', { name: 'Check current state' });
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled();
    client.admin.listProfiles.mockResolvedValue({
      items: [{ ...profileRow, retired_at: now }],
      next: null,
    });
    await user.click(screen.getByRole('checkbox'));
    await user.click(check);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(client.admin.retireProfile).toHaveBeenCalledTimes(1);
  });

  it('filters and pages activity without exposing raw detail', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.listActivity.mockResolvedValueOnce({ items: [], next: 'page-2' });
    render(<Operations client={client} capabilities={['read_activity_log']} />);
    await screen.findByText('No records match these filters.');
    await user.click(screen.getByRole('button', { name: 'Next page' }));
    await screen.findByText('Actor Morgan Park');
    expect(client.admin.listActivity).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: 'page-2' }),
    );
    await user.click(screen.getByRole('button', { name: 'Previous page' }));
    await waitFor(() =>
      expect(client.admin.listActivity).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: undefined }),
      ),
    );
    await user.type(screen.getByLabelText('Activity actor'), 'operator-2');
    await chooseOptionValue(user, screen.getByLabelText('Activity action'), 'delete');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await waitFor(() =>
      expect(client.admin.listActivity).toHaveBeenLastCalledWith(
        expect.objectContaining({ actor: 'operator-2', action: 'delete' }),
      ),
    );
  });
});
