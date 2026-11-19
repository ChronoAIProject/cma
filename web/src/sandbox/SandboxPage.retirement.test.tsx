import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { SandboxDetail } from '../api/types';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { parkedRuntimeUpdateFixture, sandboxFixture } from '../test/hierarchy';
import { SandboxPage } from './SandboxPage';
import { sandboxHeaderMenu } from './sandboxHeaderActions';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};
const reviving: SandboxDetail = {
  ...sandboxFixture('reviving'),
  lifecycle: {
    pause: { allowed: false, reason: 'work_in_progress' },
    revive: { allowed: false, reason: 'not_paused' },
    operation: {
      purpose: 'revive',
      phase: 'conversation_reopen',
      startedAt: '2026-11-18T00:00:00Z',
    },
    result: {
      state: 'in_progress',
      operationId: 'revive-fixture',
      startedAt: '2026-11-18T00:00:00Z',
      resultDueAt: '2026-11-18T00:01:30Z',
      continuing: true,
      actions: ['observe', 'retire'],
      retryAfterSeconds: 5,
    },
  },
};
const settled = { busy: false, stale: false, needsConnections: false };

it('dispatches Delete for a parked runtime update while its lifecycle request remains busy', async () => {
  const onAction = vi.fn();
  render(
    <SandboxPage
      client={agentClient({ readSandbox: async () => parkedRuntimeUpdateFixture() })}
      sandboxId="sbx_1"
      revision={0}
      permissions={permissions}
      busy={false}
      lifecycleBusy
      onAction={onAction}
      onNavigate={vi.fn()}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'More actions' }));
  const remove = screen.getByRole('menuitem', { name: /^Delete Sandbox/ });
  expect(remove).not.toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(remove);
  expect(onAction).toHaveBeenCalledExactlyOnceWith({ kind: 'delete_sandbox', sandboxId: 'sbx_1' });
});

it.each(['in_progress', 'recoverable_pending'] as const)(
  'dispatches owner Delete during %s reopen while keeping creation disabled',
  async (state) => {
    const onAction = vi.fn();
    render(
      <SandboxPage
        client={agentClient({
          readSandbox: async () => ({
            ...reviving,
            lifecycle: {
              ...reviving.lifecycle!,
              result: { ...reviving.lifecycle!.result!, state },
            },
          }),
        })}
        sandboxId="sbx_1"
        revision={0}
        permissions={permissions}
        busy={false}
        lifecycleBusy
        onAction={onAction}
        onNavigate={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'More actions' }));
    for (const name of [/^New agent/, /^New workspace/])
      expect(screen.getByRole('menuitem', { name })).toHaveAttribute('aria-disabled', 'true');
    const remove = screen.getByRole('menuitem', { name: /^Delete Sandbox/ });
    expect(remove).not.toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(remove);
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      kind: 'delete_sandbox',
      sandboxId: 'sbx_1',
    });
  },
);

it('keeps Delete disabled when the server does not advertise retirement during revive', () => {
  const sandbox = {
    ...reviving,
    lifecycle: {
      ...reviving.lifecycle!,
      result: { ...reviving.lifecycle!.result!, actions: ['observe'] as const },
    },
  };
  expect(
    sandboxHeaderMenu(sandbox, permissions, settled, false).find(
      (entry) => entry.id === 'delete_sandbox',
    ),
  ).toMatchObject({ disabled: true });
});

it.each([
  ['submission in progress', { ...settled, busy: true }],
  ['stale observation', { ...settled, stale: true }],
] as const)('keeps Delete disabled during revive with %s', (_label, context) => {
  expect(
    sandboxHeaderMenu(reviving, permissions, context, false).find(
      (entry) => entry.id === 'delete_sandbox',
    ),
  ).toMatchObject({ disabled: true });
});

it('omits Delete during revive without owner deletion permission', () => {
  expect(
    sandboxHeaderMenu(reviving, { ...permissions, deleteSandbox: false }, settled, false).some(
      (entry) => entry.id === 'delete_sandbox',
    ),
  ).toBe(false);
});
