import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { SandboxPage } from './SandboxPage';

const permissions: TreePermissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
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

it('refreshes Active saving phases and restores Pause only after authoritative eligibility clears', async () => {
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
  expect(screen.getByRole('status')).toHaveTextContent('Saving progress');
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Pause' })).toHaveAccessibleDescription(
    /Progress is being saved/,
  );
  expect(screen.queryByText(/Saving started/)).not.toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Advanced' })));
  expect(screen.getByText(/Saving started/).querySelector('time')).toHaveAttribute(
    'dateTime',
    '2026-09-08T00:00:00Z',
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Close' })));
  expect(screen.getByRole('button', { name: 'New workspace' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'New agent' })).toBeEnabled();
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByRole('status')).toHaveTextContent('Finishing save');
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
      [{ kind: 'new_agent', workspaceId: 'wsp_1' }],
    ]);
  },
);

it.each(['paused', 'reviving', 'provisioning', 'pausing', 'retiring'] as const)(
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
    expect(screen.queryByRole('button', { name: 'New agent' })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Agent one' })).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Workspaces' })).not.toBeInTheDocument();
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
