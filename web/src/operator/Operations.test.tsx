import { chooseOptionValue } from '../test/select';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { Operations, canOpenOperations } from './Operations';
import { detail, now, operatorClient, sandboxRow } from './testFixtures';
import { metricRow, reportFixture } from '../usage/metrics/testFixtures';

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
    expect(await screen.findByText('Alex Chen', { exact: true })).toBeInTheDocument();
    expect(screen.queryByText('owner-1')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Runtime details for Research Sandbox' }));
    const drawer = screen.getByRole('dialog');
    const workspace = (await within(drawer).findByText('Research Workspace: ready')).closest('li')!;
    expect(within(workspace).getByText('Research assistant: ready')).toBeInTheDocument();
    expect(within(workspace).queryByText('Review assistant: ready')).not.toBeInTheDocument();
    expect(within(drawer).getByText('Alex Chen')).toBeInTheDocument();
    expect(within(drawer).queryByText('owner-1')).not.toBeInTheDocument();
    expect(client.admin.retireSandbox).not.toHaveBeenCalled();
  });

  it('uses stable references for unresolved Activity actors without exposing their IDs', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const base = (await client.admin.listActivitySummary({})).items[0]!;
    const unresolved = {
      ...base,
      actor: 'unknown-person-id',
      actor_name: null,
      actor_reference: 'actor-0123456789abcdef',
      target: { kind: 'subject' as const, id: 'unknown-target-id' },
      target_name: null,
    };
    const service = {
      ...base,
      id: 'act-service',
      actor: 'service-principal-id',
      actor_name: null,
      actor_reference: 'actor-fedcba9876543210',
    };
    client.admin.listActivitySummary
      .mockResolvedValueOnce({
        items: [
          unresolved,
          { ...base, id: 'act-anonymous', actor: null, actor_name: null },
          service,
        ],
        next: 'page-2',
      })
      .mockResolvedValue({ items: [{ ...unresolved, id: 'act-next' }], next: null });
    render(<Operations client={client} capabilities={['read_activity_log']} />);

    const label = 'Unresolved actor · actor-0123456789abcdef';
    expect(await screen.findByRole('cell', { name: label })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: 'No actor recorded' })).toBeInTheDocument();
    expect(
      screen.getByRole('cell', { name: 'Unresolved actor · actor-fedcba9876543210' }),
    ).toBeInTheDocument();
    expect(screen.getByText('User Name unavailable')).toBeInTheDocument();
    expect(screen.queryByText('unknown-person-id')).not.toBeInTheDocument();
    expect(screen.queryByText('unknown-target-id')).not.toBeInTheDocument();
    expect(screen.queryByText('service-principal-id')).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Next page' }));
    await waitFor(() =>
      expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: 'page-2' }),
      ),
    );
    expect(await screen.findByRole('cell', { name: label })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Refresh records' }));
    expect(await screen.findByRole('cell', { name: label })).toBeInTheDocument();
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
      'Sandboxes & usage',
      'Activity',
    ]);
    expect(screen.queryByRole('tab', { name: 'Platform settings' })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Sandboxes & usage' }), {
      key: 'ArrowRight',
    });
    expect(screen.getByRole('tab', { name: 'Activity' })).toHaveFocus();
    await screen.findByRole('cell', { name: 'Morgan Park' });
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Activity' }), { key: 'Home' });
    expect(screen.getByRole('tab', { name: 'Sandboxes & usage' })).toHaveFocus();
    await screen.findByText('Research Sandbox');
    rerender(<Operations client={client} capabilities={['read_activity_log']} />);
    await screen.findByRole('cell', { name: 'Morgan Park' });
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    expect(client.admin.readSettings).not.toHaveBeenCalled();
    expect(client.admin.listProfiles).not.toHaveBeenCalled();
    expect(client.admin.retireProfile).not.toHaveBeenCalled();
    expect(screen.queryByText('must not render')).not.toBeInTheDocument();
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
    const trigger = await screen.findByRole('button', {
      name: 'Runtime details for Research Sandbox',
    });
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
    expect(client.adminUsageMetrics.create).toHaveBeenCalledTimes(2);
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

  it('filters and pages activity without exposing raw detail', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.listActivitySummary.mockResolvedValueOnce({ items: [], next: 'page-2' });
    render(<Operations client={client} capabilities={['read_activity_log']} />);
    await screen.findByText('No records match these filters.');
    await user.click(screen.getByRole('button', { name: 'Next page' }));
    await screen.findByRole('cell', { name: 'Morgan Park' });
    expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: 'page-2' }),
    );
    await user.click(screen.getByRole('button', { name: 'Previous page' }));
    await waitFor(() =>
      expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: undefined }),
      ),
    );
    // Actor is a selector populated from the canonical actor names returned by
    // the activity endpoint; operators no longer need to type a subject id.
    await chooseOptionValue(user, screen.getByLabelText('Activity actor'), 'operator-1');
    await chooseOptionValue(user, screen.getByLabelText('Activity action'), 'delete');
    await user.click(screen.getByRole('button', { name: 'Apply filters' }));
    await waitFor(() =>
      expect(client.admin.listActivitySummary).toHaveBeenLastCalledWith(
        expect.objectContaining({ actor: 'operator-1', action: 'delete' }),
      ),
    );
  });
});

describe('operator record tables', () => {
  const headers = (name: string) =>
    within(screen.getByRole('table', { name }))
      .getAllByRole('columnheader')
      .map((header) => header.textContent);

  it('retains runtime controls beside allocated columns and performs no inventory sweep', async () => {
    const client = operatorClient();
    client.adminUsageMetrics.sandboxes.mockResolvedValue({
      report_id: 'report-one',
      as_of: 1,
      items: [metricRow, { ...metricRow, sandbox_id: 'sbx-2', name: 'Second', status: 'failed' }],
      next: null,
      total_count: 2,
    });
    render(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    await screen.findByRole('cell', { name: /Second/ });
    expect(headers('Sandboxes and allocated usage')).toContain('CPU (core-hours)');
    for (const button of screen.getAllByRole('button', { name: 'Retire' }))
      expect(button).toBeDisabled();
    expect(client.admin.listSandboxes).not.toHaveBeenCalled();
    expect(client.usage.adminSandboxes).not.toHaveBeenCalled();
    expect(client.adminUsageMetrics.sandboxes).toHaveBeenCalledTimes(1);
  });
  it('keeps a failed metrics report from claiming an empty inventory', async () => {
    const client = operatorClient();
    client.adminUsageMetrics.create.mockResolvedValue({
      ...reportFixture(true),
      status: 'failed',
      summary: null,
    });
    render(<Operations client={client} capabilities={['read_all_sandboxes']} />);
    await screen.findByText('Could not load usage');
    expect(screen.queryByText('No Sandboxes match this selection.')).not.toBeInTheDocument();
    expect(client.admin.listSandboxes).not.toHaveBeenCalled();
  });

  it('draws each Activity outcome in its own tone under an SGT time column', async () => {
    const client = operatorClient();
    const base = (await client.admin.listActivitySummary({})).items[0]!;
    client.admin.listActivitySummary.mockResolvedValue({
      items: [
        base,
        { ...base, id: 'act-2', outcome: { status: 'refused', code: 'quota_exceeded' } },
        { ...base, id: 'act-3', outcome: { status: 'failed', code: 'internal' } },
      ],
      next: null,
    });
    render(<Operations client={client} capabilities={['read_activity_log']} />);
    await screen.findByText('Refused');
    expect(headers('Activity')).toEqual([
      'Time (SGT)',
      'Actor',
      'Channel',
      'Action',
      'Target',
      'Outcome',
      'Code',
      'Request ID',
    ]);
    const tones = ['Succeeded', 'Refused', 'Failed'].map((word) =>
      screen.getByText(word).closest('.badge')?.getAttribute('data-tone'),
    );
    expect(tones).toEqual(['neutral', 'warning', 'error']);
    expect(screen.getByRole('cell', { name: 'quota_exceeded' })).toHaveAttribute('data-mono');
  });
});
