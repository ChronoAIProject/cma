import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createClient } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { NamedReferences } from '../test/references';
import { SandboxPage } from './SandboxPage';

const permissions: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

function openMoreActions() {
  fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
}

/** A More actions row; its accessible name also carries any unavailable reason. */
function menuItem(name: string) {
  return screen.getByRole('menuitem', { name: new RegExp(`^${name}`) });
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function lifecyclePage(readSandbox: () => Promise<SandboxDetail>) {
  return render(
    <SandboxPage
      client={agentClient({ readSandbox })}
      sandboxId="sbx_1"
      revision={0}
      permissions={{ ...permissions, createWorkspace: true, createAgent: true }}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
}

const savingSandbox = (phase: 'capture' | 'resume_writers'): SandboxDetail => ({
  ...sandboxFixture(),
  lifecycle: {
    pause: { allowed: false, reason: 'checkpoint_in_progress' },
    revive: { allowed: false, reason: 'not_paused' },
    checkpoint: { phase, startedAt: '2026-09-08T00:00:00Z', epoch: 7 },
  },
});

it.each(['pausing', 'reviving', 'retiring'] as const)(
  'reads %s activity only once the Activity tab is opened',
  async (status) => {
    const readSandboxEvents = vi.fn(async () => []);
    render(
      <SandboxPage
        client={agentClient({ readSandbox: async () => sandboxFixture(status), readSandboxEvents })}
        sandboxId="sbx_1"
        revision={0}
        permissions={permissions}
        busy={false}
        onAction={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    const tab = await screen.findByRole('tab', { name: 'Activity' });
    expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
    expect(readSandboxEvents).not.toHaveBeenCalled();
    fireEvent.click(tab);
    expect(await screen.findByRole('region', { name: 'Sandbox progress' })).toBeInTheDocument();
    expect(readSandboxEvents).toHaveBeenCalled();
  },
);

it('explains provider persistence while workspace, pause and delete actions remain usable', async () => {
  lifecyclePage(async () => ({
    ...sandboxFixture(),
    lifecycle: {
      pause: { allowed: true },
      revive: { allowed: false, reason: 'not_paused' },
      backgroundSave: {
        phase: 'exporting',
        startedAt: '2026-09-10T00:00:00Z',
        capturedAt: '2026-09-10T00:00:02Z',
        epoch: 8,
      },
    },
  }));
  fireEvent.click(await screen.findByRole('tab', { name: 'Activity' }));
  expect(
    await screen.findByText(/keeps its files and installed software on its saved home/),
  ).toBeInTheDocument();
  expect(screen.queryByText(/S3 transfers/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  openMoreActions();
  expect(menuItem('New workspace')).not.toHaveAttribute('aria-disabled', 'true');
  expect(menuItem('Delete Sandbox')).not.toHaveAttribute('aria-disabled', 'true');
  expect(screen.queryByText('Profile')).not.toBeInTheDocument();
  expect(screen.queryByText('Profile:')).not.toBeInTheDocument();
  expect(document.querySelector('a[href^="/profiles/"]')).toBeNull();
});

it.each(['active', 'paused'] as const)(
  'shows the creator name on the %s Sandbox page without opening Advanced',
  async (status) => {
    lifecyclePage(async () => ({
      ...sandboxFixture(),
      status,
      owner: 'creator-private-subject',
      ownerName: 'Sam Lee',
    }));
    const owner = await screen.findByText('Owner');
    expect(owner.parentElement?.querySelector('dd')).toHaveTextContent('Sam Lee');
    expect(screen.queryByText('creator-private-subject')).not.toBeInTheDocument();
  },
);

it('does not replace a missing creator name with their subject ID', async () => {
  lifecyclePage(async () => ({ ...sandboxFixture(), owner: 'creator-private-subject' }));
  expect(await screen.findByText('Name unavailable')).toBeInTheDocument();
  expect(screen.queryByText('creator-private-subject')).not.toBeInTheDocument();
});

it.each(['https://catalog.example.test', 'https://second.example.test'])(
  'links a paused Sandbox’s services through %s without relying on Agent reads',
  async (catalogOrigin) => {
    render(
      <NamedReferences catalogOrigin={catalogOrigin}>
        <SandboxPage
          client={agentClient({
            readSandbox: async () => ({
              ...sandboxFixture('paused'),
              nyxidServices: ['github', 'storage'],
              workspaces: [],
            }),
            readAgent: async () => {
              throw new Error('Agent unavailable');
            },
          })}
          sandboxId="sbx_1"
          revision={0}
          permissions={permissions}
          busy={false}
          onAction={vi.fn()}
          onNavigate={vi.fn()}
        />
      </NamedReferences>,
    );
    fireEvent.click(await screen.findByRole('tab', { name: 'Access' }));
    expect(await screen.findByRole('link', { name: 'Chrono Storage' })).toHaveAttribute(
      'href',
      `${catalogOrigin}/services/catalog-storage`,
    );
    expect(screen.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
      'href',
      `${catalogOrigin}/services/catalog-github`,
    );
    expect(screen.queryByText('Access not yet confirmed')).not.toBeInTheDocument();
    expect(screen.queryByText('Requested services')).not.toBeInTheDocument();
  },
);

it('explains persistent PV storage in Technical details and exposes no snapshot controls', async () => {
  const sandbox = savingSandbox('capture');
  lifecyclePage(async () => ({
    ...sandbox,
    lifecycle: sandbox.lifecycle,
  }));
  await act(async () => {});
  expect(screen.queryByText('Latest snapshot')).not.toBeInTheDocument();
  openMoreActions();
  await act(async () => fireEvent.click(menuItem('Technical details')));
  expect(screen.getByRole('dialog', { name: 'Technical details' })).toBeInTheDocument();
  expect(screen.getByText('Persistent storage')).toBeInTheDocument();
  expect(
    screen.getByText(/CMA does not create, schedule, download or delete S3 snapshots/),
  ).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: 'Download snapshot' })).not.toBeInTheDocument();
});

it('refreshes lifecycle phases and restores Pause only after authoritative eligibility clears', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(sandboxFixture())
    .mockResolvedValueOnce(savingSandbox('capture'))
    .mockResolvedValueOnce(savingSandbox('resume_writers'))
    .mockResolvedValue(sandboxFixture());
  lifecyclePage(readSandbox);
  await act(async () => {});
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(
    screen.getByText('Live', { selector: '[role="status"] .badge__word' }),
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Pause' })).toHaveAccessibleDescription(
    /Progress is being saved/,
  );
  expect(screen.queryByText(/Saving started/)).not.toBeInTheDocument();
  openMoreActions();
  await act(async () => fireEvent.click(menuItem('Technical details')));
  expect(
    screen.getByText(/CMA does not create, schedule, download or delete S3 snapshots/),
  ).toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Close' })));
  openMoreActions();
  expect(menuItem('New agent')).not.toHaveAttribute('aria-disabled', 'true');
  expect(menuItem('New workspace')).not.toHaveAttribute('aria-disabled', 'true');
  fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(
    screen.getByText('Live', { selector: '[role="status"] .badge__word' }),
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  expect(screen.queryByText(/Saving started/)).not.toBeInTheDocument();
  expect(readSandbox).toHaveBeenCalledTimes(4);
});

it('keeps loaded content on a failed refresh but holds stale Pause eligibility until recovery', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(sandboxFixture())
    .mockRejectedValueOnce(new Error('temporarily unavailable'))
    .mockResolvedValue(sandboxFixture());
  lifecyclePage(readSandbox);
  await act(async () => {});
  fireEvent.click(screen.getByRole('tab', { name: 'Workspaces' }));
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Agent one' })).toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent('Reconnecting');
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
});

it('does not overlap reads and defers hidden-page refresh until visible', async () => {
  vi.useFakeTimers();
  let resolveRead: (sandbox: SandboxDetail) => void = () => {};
  const readSandbox = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<SandboxDetail>((resolve) => {
          resolveRead = resolve;
        }),
    )
    .mockResolvedValue(sandboxFixture());
  const view = lifecyclePage(readSandbox);
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolveRead(sandboxFixture());
  });
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  hidden.mockReturnValue(false);
  await act(async () => {
    document.dispatchEvent(new Event('visibilitychange'));
  });
  expect(readSandbox).toHaveBeenCalledTimes(2);
  view.unmount();
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(readSandbox).toHaveBeenCalledTimes(2);
});

it.each(['active', 'paused'] as const)(
  'holds %s lifecycle controls when eligibility is unreported',
  async (status) => {
    lifecyclePage(async () => ({ ...sandboxFixture(status), lifecycle: undefined }));
    expect(
      await screen.findByRole('button', { name: status === 'paused' ? 'Resume' : 'Pause' }),
    ).toBeDisabled();
    expect(screen.getByText('Action availability is not reported.')).toBeInTheDocument();
  },
);

it.each(['active'] as const)(
  'allows normal child creation from a %s parent without an explicit revive action',
  async (status) => {
    const onAction = vi.fn();
    render(
      <SandboxPage
        client={agentClient({ readSandbox: async () => sandboxFixture(status) })}
        sandboxId="sbx_1"
        revision={0}
        permissions={{ ...permissions, createWorkspace: true, createAgent: true }}
        busy={false}
        onAction={onAction}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole('heading', { name: 'Atlas' });
    expect(
      screen.queryByRole('button', { name: 'New agent in this Sandbox' }),
    ).not.toBeInTheDocument();
    openMoreActions();
    expect(menuItem('New agent')).not.toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(menuItem('New workspace'));
    openMoreActions();
    fireEvent.click(menuItem('New agent'));
    fireEvent.click(screen.getByRole('tab', { name: 'Workspaces' }));
    const here = await screen.findByRole('button', { name: 'New agent in chrono-bot' });
    expect(here).toBeEnabled();
    here.click();
    expect(onAction.mock.calls).toEqual([
      [{ kind: 'new_workspace', sandboxId: 'sbx_1' }],
      [{ kind: 'new_agent', sandboxId: 'sbx_1' }],
      [{ kind: 'new_agent', workspaceId: 'wsp_1' }],
    ]);
  },
);

it.each(['paused', 'reviving', 'rotating', 'provisioning', 'pausing', 'retiring'] as const)(
  'keeps child creation disabled for a %s parent',
  async (status) => {
    render(
      <SandboxPage
        client={agentClient({ readSandbox: async () => sandboxFixture(status) })}
        sandboxId="sbx_1"
        revision={0}
        permissions={{ ...permissions, createWorkspace: true, createAgent: true }}
        busy={false}
        onAction={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole('heading', { name: 'Atlas' });
    openMoreActions();
    expect(menuItem('New agent')).toHaveAttribute('aria-disabled', 'true');
    expect(menuItem('New workspace')).toHaveAttribute('aria-disabled', 'true');
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
    fireEvent.click(screen.getByRole('tab', { name: 'Workspaces' }));
    if (['paused', 'pausing', 'reviving', 'rotating'].includes(status)) {
      expect(await screen.findByRole('link', { name: 'Agent one' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'New agent in chrono-bot' })).toBeDisabled();
    } else {
      expect(
        screen.queryByRole('button', { name: 'New agent in chrono-bot' }),
      ).not.toBeInTheDocument();
      expect(screen.queryByRole('link', { name: 'Agent one' })).not.toBeInTheDocument();
      expect(
        screen.getByText(
          status === 'retiring'
            ? 'Workspaces are no longer available in this Sandbox.'
            : 'Workspaces appear here when the Sandbox is ready.',
        ),
      ).toBeInTheDocument();
    }
    if (status === 'paused') {
      expect(screen.getByRole('button', { name: 'Resume' })).toBeEnabled();
      expect(screen.getByText(/Your work is preserved/)).toBeInTheDocument();
    }
  },
);

it.each(['atlas', 'boreal'])(
  'opens the retained failed repository Workspace for %s while creation is disabled',
  async (name) => {
    const sandboxId = `sbx_${name}`;
    const workspace = {
      ...workspaceFixture(),
      id: `wsp_${name}`,
      sandboxId,
      name: `${name}-repository`,
      status: 'failed' as const,
      path: `/workspaces/${name}`,
      agents: [],
      worktrees: [],
      agentCount: 0,
      worktreeCount: 0,
    };
    const sandbox = { ...sandboxFixture('failed'), id: sandboxId, workspaces: [workspace] };
    const onNavigate = vi.fn();
    render(
      <SandboxPage
        client={agentClient({
          readSandbox: async () => sandbox,
          readWorkspace: async () => workspace,
        })}
        sandboxId={sandboxId}
        revision={0}
        permissions={{ ...permissions, createWorkspace: true, createAgent: true }}
        busy={false}
        onAction={vi.fn()}
        onNavigate={onNavigate}
      />,
    );
    fireEvent.click(await screen.findByRole('tab', { name: 'Workspaces' }));
    const link = await screen.findByRole('link', { name: workspace.name });
    expect(link).toHaveAttribute('href', `/workspaces/${workspace.id}`);
    expect(link.closest('tr')).toHaveTextContent('Needs attention');
    expect(link.closest('tr')).toHaveTextContent(workspace.path);
    expect(screen.getByRole('button', { name: 'New workspace' })).toBeDisabled();
    expect(screen.getByRole('button', { name: `New agent in ${workspace.name}` })).toBeDisabled();
    fireEvent.click(link);
    expect(onNavigate).toHaveBeenCalledWith({ kind: 'workspace', workspaceId: workspace.id });
  },
);

it('pages the complete Sandbox event timeline before displaying it', async () => {
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValueOnce([
      { sandboxId: 'sbx_1', seq: 1, appendedAt: '2026-09-06T00:00:00Z', kind: { type: 'created' } },
    ])
    .mockResolvedValueOnce([
      {
        sandboxId: 'sbx_1',
        seq: 2,
        appendedAt: '2026-09-06T00:01:00Z',
        kind: { type: 'workspace_created' },
      },
    ])
    .mockResolvedValueOnce([]);
  render(
    <SandboxPage
      client={agentClient({ readSandboxEvents })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'More actions' }));
  const technical = menuItem('Technical details');
  expect(readSandboxEvents).not.toHaveBeenCalled();
  fireEvent.click(technical);
  expect(await screen.findByText('Workspace created')).toBeInTheDocument();
  expect(screen.getByText('Sandbox created')).toBeInTheDocument();
  expect(readSandboxEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 1, 2]);
});

it('fails closed for a nonadvancing Sandbox event page', async () => {
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValue([
      { sandboxId: 'sbx_1', seq: 1, appendedAt: '2026-09-06T00:00:00Z', kind: { type: 'created' } },
    ]);
  render(
    <SandboxPage
      client={agentClient({ readSandboxEvents })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'More actions' }));
  fireEvent.click(menuItem('Technical details'));
  expect(await screen.findByRole('alert')).toHaveTextContent('Sandbox events did not advance.');
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeInTheDocument();
  expect(readSandboxEvents).toHaveBeenCalledTimes(2);
});

function sharingViews(capabilities: readonly string[]) {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname;
    if (path.includes('/resource-access/'))
      return Response.json({ capabilities, revalidate_at: null, policy_revision: 1 });
    if (path.endsWith('/organizations')) return Response.json({ items: [] });
    if (path.includes('/resource-shares/')) return Response.json({ items: [] });
    throw new Error(`Unexpected resource request ${path}`);
  });
  return { fetch, client: createClient({ baseUrl: 'https://fixture.invalid', fetch }) };
}

it.each([
  [['view_details', 'manage_sharing'], true],
  [['view_details'], false],
] as const)(
  'lists Sharing in More actions only with current authority (%j)',
  async (capabilities, allowed) => {
    const views = sharingViews(capabilities);
    render(
      <SandboxPage
        client={agentClient({ resourceViews: views.client })}
        sandboxId="sbx_1"
        revision={0}
        permissions={permissions}
        busy={false}
        onAction={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    await screen.findByRole('button', { name: 'More actions' });
    await waitFor(() => expect(views.fetch).toHaveBeenCalled());
    await act(async () => {});
    openMoreActions();
    if (!allowed) {
      expect(screen.queryByRole('menuitem', { name: 'Sharing' })).not.toBeInTheDocument();
      return;
    }
    fireEvent.click(menuItem('Sharing'));
    expect(await screen.findByRole('dialog', { name: 'Sandbox sharing' })).toBeInTheDocument();
    expect(screen.getByText('Sharing applies to the whole Sandbox.')).toBeInTheDocument();
  },
);

it('deletes from the last More actions row and explains held header actions', async () => {
  const onAction = vi.fn();
  render(
    <SandboxPage
      client={agentClient({
        readSandbox: async () => ({
          ...sandboxFixture(),
          lifecycle: {
            pause: { allowed: false, reason: 'work_in_progress' },
            revive: { allowed: false, reason: 'not_paused' },
            operation: { purpose: 'pause', phase: 'capture', startedAt: '2026-10-21T00:00:00Z' },
          },
        }),
      })}
      sandboxId="sbx_1"
      revision={0}
      permissions={{ ...permissions, createAgent: true, createWorkspace: true }}
      busy={false}
      onAction={onAction}
      onNavigate={vi.fn()}
    />,
  );
  await screen.findByRole('heading', { name: 'Atlas' });
  expect(screen.getByRole('button', { name: 'Pause' })).toHaveAccessibleDescription(
    'Sandbox work is in progress. This action will be available when it finishes.',
  );
  openMoreActions();
  const agent = menuItem('New agent');
  expect(agent).toHaveAttribute('aria-disabled', 'true');
  expect(agent).toHaveTextContent('Wait for the current Sandbox operation to finish.');
  fireEvent.click(agent);
  const items = screen.getAllByRole('menuitem');
  expect(items.at(-1)).toBe(menuItem('Delete Sandbox'));
  expect(menuItem('Delete Sandbox')).toHaveAttribute('aria-disabled', 'true');
  expect(menuItem('Delete Sandbox')).toHaveTextContent(
    'Wait for the current Sandbox operation to finish.',
  );
  fireEvent.click(menuItem('Delete Sandbox'));
  expect(onAction).not.toHaveBeenCalled();
});

it('dispatches Delete Sandbox for an available Sandbox', async () => {
  const onAction = vi.fn();
  render(
    <SandboxPage
      client={agentClient()}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={onAction}
      onNavigate={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'More actions' }));
  fireEvent.click(menuItem('Delete Sandbox'));
  expect(onAction).toHaveBeenCalledWith({ kind: 'delete_sandbox', sandboxId: 'sbx_1' });
});
