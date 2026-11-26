import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail, SandboxRuntimeUpdate } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { SandboxPage } from './SandboxPage';

vi.mock('../connections/ConnectionWizard', () => ({ ConnectionWizard: () => null }));
afterEach(() => vi.useRealTimers());

const update: SandboxRuntimeUpdate = {
  currentVersion: 'registry.example/runtime:old',
  latestVersion: 'registry.example/runtime:new',
  updateAvailable: true,
  state: 'available',
  canUpdate: true,
  schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
  preservesHome: true,
};
const permissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: false,
  deleteAgent: false,
  exportHistory: true,
};

it('retains accepted status and hierarchy through older reads, using the existing refresh loop', async () => {
  vi.useFakeTimers();
  let sandbox: SandboxDetail = { ...sandboxFixture(), owner: 'owner', runtimeUpdate: update };
  const readSandbox = vi.fn(async () => sandbox);
  const updateSandboxRuntime = vi.fn(async () => ({
    revision: 6,
    runtimeUpdate: { ...update, state: 'queued' as const, canUpdate: false },
  }));
  render(
    <SandboxPage
      client={agentClient({ readSandbox, updateSandboxRuntime })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      viewerId="owner"
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  await act(async () => {});
  const agent = screen.getByRole('link', { name: 'Agent one', hidden: true });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
  expect(screen.getByText('Runtime update queued')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Agent one', hidden: true })).toBe(agent);
  expect(readSandbox).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByText('Runtime update queued')).toBeVisible();
  expect(readSandbox).toHaveBeenCalledTimes(2);
  sandbox = {
    ...sandboxFixture('rotating'),
    owner: 'owner',
    revision: 7,
    runtimeUpdate: { ...update, state: 'updating', canUpdate: false },
  };
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByText('Updating your runtime')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Agent one', hidden: true })).toBe(agent);
  expect(readSandbox).toHaveBeenCalledTimes(3);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(readSandbox).toHaveBeenCalledTimes(4);
});

it('does not expose runtime update controls to a viewer who is not the owner', async () => {
  const updateSandboxRuntime = vi.fn();
  render(
    <SandboxPage
      client={agentClient({
        readSandbox: async () => ({ ...sandboxFixture(), owner: 'owner', runtimeUpdate: update }),
        updateSandboxRuntime,
      })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      viewerId="different-owner"
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  await act(async () => {});
  expect(screen.queryByRole('region', { name: 'Runtime' })).not.toBeInTheDocument();
  expect(updateSandboxRuntime).not.toHaveBeenCalled();
});

it('allows retry of the retained failed target while the Sandbox remains rotating', async () => {
  const targetVersion = 'registry.example/runtime:requested';
  const updateSandboxRuntime = vi.fn(async () => ({
    revision: 6,
    runtimeUpdate: { ...update, targetVersion, state: 'updating' as const, canUpdate: false },
  }));
  render(
    <SandboxPage
      client={agentClient({
        readSandbox: async () => ({
          ...sandboxFixture('rotating'),
          owner: 'owner',
          runtimeUpdate: { ...update, targetVersion, state: 'failed' },
        }),
        updateSandboxRuntime,
      })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      viewerId="owner"
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  await act(async () => {});
  const agent = screen.getByRole('link', { name: 'Agent one', hidden: true });
  const retry = screen.getByRole('button', { name: 'Retry runtime update' });
  expect(retry).toBeEnabled();
  await act(async () => fireEvent.click(retry));
  expect(updateSandboxRuntime).toHaveBeenCalledWith('sbx_1', targetVersion, {
    idempotencyKey: expect.any(String),
    signal: expect.any(AbortSignal),
  });
  expect(screen.getByText('Updating your runtime')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Agent one', hidden: true })).toBe(agent);
});

const currentRuntime: SandboxRuntimeUpdate = {
  ...update,
  currentVersion: update.latestVersion,
  state: 'up_to_date',
  updateAvailable: false,
  canUpdate: false,
  completedAt: '2026-11-22T18:00:00Z',
};

function ownerPage(client: ReturnType<typeof agentClient>, busy = false) {
  return (
    <SandboxPage
      client={client}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={busy}
      viewerId="owner"
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />
  );
}

it.each([
  'Wait for the current Sandbox operation to finish.',
  'Runtime updates are blocked by the Sandbox failure. Open Technical details for the reported failure.',
])(
  'shows the reported failure and disables updates when a newer runtime is available (%s)',
  async (blockedReason) => {
    const updateSandboxRuntime = vi.fn();
    render(
      ownerPage(
        agentClient({
          readSandbox: async () => ({
            ...sandboxFixture('failed'),
            owner: 'owner',
            failure: { class: 'terminal_validation', detail: 'Synthetic failure' },
            runtimeUpdate: { ...update, canUpdate: false, blockedReason },
          }),
          updateSandboxRuntime,
        }),
      ),
    );
    expect(await screen.findByText('Your sandbox needs attention.')).toBeVisible();
    const runtime = screen.getByRole('region', { name: 'Runtime' });
    expect(runtime).toHaveTextContent('Runtime update unavailable');
    expect(runtime).toHaveTextContent('A newer managed runtime version is available.');
    expect(runtime).not.toHaveTextContent('Update now');
    expect(runtime).not.toHaveTextContent('Wait for the current Sandbox operation to finish.');
    const action = screen.getByRole('button', { name: 'Update runtime' });
    expect(action).toBeDisabled();
    expect(action).toHaveAccessibleDescription(
      'Runtime updates are blocked by the Sandbox failure. Open Technical details for the reported failure.',
    );
    fireEvent.click(action);
    expect(updateSandboxRuntime).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText('Technical details', { selector: 'summary' }));
    expect(screen.getByText('Unavailable while the Sandbox needs attention')).toBeVisible();
    expect(screen.getByText(update.currentVersion!)).toBeVisible();
    expect(screen.getByText(update.latestVersion)).toBeVisible();
  },
);

it.each([
  ['active', currentRuntime, 'Runtime updated', 'done'],
  ['active', { ...currentRuntime, currentVersion: null }, 'Runtime version unconfirmed', 'neutral'],
  [
    'active',
    { ...currentRuntime, currentVersion: update.currentVersion },
    'Runtime version unconfirmed',
    'neutral',
  ],
  ['paused', currentRuntime, 'Runtime version unconfirmed', 'neutral'],
  ['retiring', currentRuntime, 'Environment retiring', 'neutral'],
  ['retired', currentRuntime, 'Environment retired', 'neutral'],
] as const)(
  'presents %s lifecycle before retained runtime health (%s)',
  async (status, runtimeUpdate, title, tone) => {
    const updateSandboxRuntime = vi.fn();
    render(
      ownerPage(
        agentClient({
          readSandbox: async () => ({ ...sandboxFixture(status), owner: 'owner', runtimeUpdate }),
          updateSandboxRuntime,
        }),
      ),
    );
    expect(await screen.findByText(title, { exact: true })).toBeVisible();
    const runtime = screen.getByRole('region', { name: 'Runtime' });
    expect(runtime.querySelector('.statedot')).toHaveAttribute('data-state', tone);
    expect(screen.queryByText('Runtime up to date')).not.toBeInTheDocument();
    if (status === 'retired' || status === 'retiring')
      expect(
        screen.queryByText('Your existing home disk, files and conversations are preserved.'),
      ).not.toBeInTheDocument();
    if (status === 'retired') expect(runtime).toHaveTextContent('no longer running');
    expect(screen.getByText('sbx_1', { selector: 'code' })).not.toBeVisible();
    expect(
      screen
        .getAllByText(update.latestVersion, { selector: 'code' })
        .every((node) => !node.closest('details')?.open),
    ).toBe(true);
    fireEvent.click(screen.getByText('Technical details', { selector: 'summary' }));
    expect(screen.getByText('sbx_1', { selector: 'code' })).toBeVisible();
    expect(updateSandboxRuntime).not.toHaveBeenCalled();
  },
);

it('removes the healthy runtime claim after a read fails and retains diagnostic facts', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce({ ...sandboxFixture(), owner: 'owner', runtimeUpdate: currentRuntime })
    .mockRejectedValue(new Error('Read unavailable'));
  render(ownerPage(agentClient({ readSandbox })));
  await act(async () => {});
  expect(screen.getByText('Runtime updated')).toBeVisible();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByText('Runtime status unavailable')).toBeVisible();
  expect(screen.queryByText('Runtime updated')).not.toBeInTheDocument();
  expect(
    screen.getByRole('region', { name: 'Runtime' }).querySelector('.statedot'),
  ).toHaveAttribute('data-state', 'warning');
  fireEvent.click(screen.getByText('Technical details', { selector: 'summary' }));
  expect(
    screen.getAllByText(update.latestVersion).every((node) => node.closest('details')?.open),
  ).toBe(true);
});

it('retains an unconfirmed update through retirement and retries only the original target and key', async () => {
  vi.useFakeTimers();
  let sandbox: SandboxDetail = { ...sandboxFixture(), owner: 'owner', runtimeUpdate: update };
  const updateSandboxRuntime = vi.fn().mockRejectedValue(new Error('Response lost'));
  const client = agentClient({ readSandbox: async () => sandbox, updateSandboxRuntime });
  const view = render(ownerPage(client));
  await act(async () => {});
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
  const original = updateSandboxRuntime.mock.calls[0]!;
  expect(screen.getByText('Runtime update unconfirmed')).toBeVisible();
  sandbox = {
    ...sandboxFixture('retired'),
    owner: 'owner',
    revision: 7,
    runtimeUpdate: { ...currentRuntime, latestVersion: 'registry.example/runtime:newer' },
  };
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByText('Environment retired')).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent('The update request is not yet confirmed.');
  view.rerender(ownerPage(client, true));
  expect(screen.getByRole('button', { name: 'Retry update request' })).toBeDisabled();
  view.rerender(ownerPage(client));
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Retry update request' })),
  );
  expect(updateSandboxRuntime).toHaveBeenCalledTimes(2);
  expect(updateSandboxRuntime.mock.calls[1]).toEqual([
    original[0],
    original[1],
    { ...original[2], signal: expect.any(AbortSignal) },
  ]);
});
