import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail, SandboxRuntimeUpdate } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { SandboxPage } from './SandboxPage';

vi.mock('../connections/ConnectionWizard', () => ({ ConnectionWizard: () => null }));
vi.mock('../landing/LandingCharacter', () => ({ LandingCharacter: () => null }));
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
  const agent = screen.getByRole('link', { name: 'Agent one' });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Update runtime' })));
  expect(screen.getByText('Runtime update queued')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Agent one' })).toBe(agent);
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
  expect(screen.getByRole('link', { name: 'Agent one' })).toBe(agent);
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
  const agent = screen.getByRole('link', { name: 'Agent one' });
  const retry = screen.getByRole('button', { name: 'Retry runtime update' });
  expect(retry).toBeEnabled();
  await act(async () => fireEvent.click(retry));
  expect(updateSandboxRuntime).toHaveBeenCalledWith('sbx_1', targetVersion, {
    idempotencyKey: expect.any(String),
    signal: expect.any(AbortSignal),
  });
  expect(screen.getByText('Updating your runtime')).toBeVisible();
  expect(screen.getByRole('link', { name: 'Agent one' })).toBe(agent);
});
