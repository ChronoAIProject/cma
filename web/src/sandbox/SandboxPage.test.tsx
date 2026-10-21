import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
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
  'keeps underlying %s activity closed until requested',
  async (status) => {
    lifecyclePage(async () => sandboxFixture(status));
    const disclosure = await screen.findByText('Sandbox operation progress');
    expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
    fireEvent.click(disclosure);
    expect(await screen.findByRole('region', { name: 'Sandbox progress' })).toBeInTheDocument();
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
  expect(
    await screen.findByText(/keeps its files and installed software on its saved home/),
  ).toBeInTheDocument();
  expect(screen.queryByText(/S3 transfers/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'New workspace' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Delete' })).toBeEnabled();
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

it('shows granted service names for a paused Sandbox without relying on Agent reads', async () => {
  render(
    <NamedReferences>
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
  expect(await screen.findByRole('link', { name: 'Chrono Storage' })).toHaveAttribute(
    'href',
    'https://nyx.chrono-ai.fun/services/catalog-storage',
  );
  expect(screen.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
    'href',
    'https://nyx.chrono-ai.fun/services/catalog-github',
  );
  expect(screen.queryByText('Access not yet confirmed')).not.toBeInTheDocument();
  expect(screen.queryByText('Requested services')).not.toBeInTheDocument();
});

it('explains persistent PV storage in Advanced and exposes no snapshot controls', async () => {
  const sandbox = savingSandbox('capture');
  lifecyclePage(async () => ({
    ...sandbox,
    lifecycle: sandbox.lifecycle,
  }));
  await act(async () => {});
  expect(screen.queryByText('Latest snapshot')).not.toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Advanced' })));
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
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Advanced' })));
  expect(
    screen.getByText(/CMA does not create, schedule, download or delete S3 snapshots/),
  ).toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Close' })));
  expect(screen.getByRole('button', { name: 'New workspace' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'New agent' })).toBeEnabled();
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
    const workspace = await screen.findByRole('button', { name: 'New workspace' });
    const agent = await screen.findByRole('button', { name: 'New agent' });
    expect(workspace).toBeEnabled();
    expect(agent).toBeEnabled();
    workspace.click();
    agent.click();
    expect(onAction.mock.calls).toEqual([
      [{ kind: 'new_workspace', sandboxId: 'sbx_1' }],
      [{ kind: 'new_agent', sandboxId: 'sbx_1' }],
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
    expect(await screen.findByRole('button', { name: 'New workspace' })).toBeDisabled();
    if (['paused', 'pausing', 'reviving', 'rotating'].includes(status)) {
      expect(await screen.findByRole('link', { name: 'Agent one' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'New agent' })).toBeDisabled();
      expect(screen.getByRole('heading', { name: 'Workspaces' })).toBeInTheDocument();
    } else {
      expect(screen.queryByRole('button', { name: 'New agent' })).not.toBeInTheDocument();
      expect(screen.queryByRole('link', { name: 'Agent one' })).not.toBeInTheDocument();
      expect(screen.queryByRole('heading', { name: 'Workspaces' })).not.toBeInTheDocument();
    }
    if (status === 'paused') {
      expect(screen.getByRole('button', { name: 'Resume' })).toBeEnabled();
      expect(screen.getByText(/Your work is preserved/)).toBeInTheDocument();
    }
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
  const advanced = await screen.findByRole('button', { name: 'Advanced' });
  expect(readSandboxEvents).not.toHaveBeenCalled();
  fireEvent.click(advanced);
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
  fireEvent.click(await screen.findByRole('button', { name: 'Advanced' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Sandbox events did not advance.');
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeInTheDocument();
  expect(readSandboxEvents).toHaveBeenCalledTimes(2);
});
