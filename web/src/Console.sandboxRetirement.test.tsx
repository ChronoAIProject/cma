import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { SandboxDetail } from './api/types';
import { Console } from './Console';
import { connectionClient } from './connections/client';
import { agentClient } from './test/agent';
import { parkedRuntimeUpdateFixture, sandboxFixture } from './test/hierarchy';

afterEach(() => vi.restoreAllMocks());

it('lets an owner delete a parked update and retains asynchronous cleanup status', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-11-19T00:00:00Z',
    services: [],
  });
  let sandbox = parkedRuntimeUpdateFixture();
  const deleteSandbox = vi.fn(async () => {
    sandbox = sandboxFixture('retiring');
  });
  const updateSandboxRuntime = vi.fn();
  const client = agentClient({
    readSandbox: async () => sandbox,
    deleteSandbox,
    updateSandboxRuntime,
  });
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={client}>
      <Console
        user={{
          id: 'owner',
          handle: 'Owner',
          visitor: false,
          capabilities: ['use_own_sandboxes', 'delete_own_sandbox'],
        }}
        route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await actor.click(await screen.findByRole('button', { name: 'More actions' }));
  await actor.click(screen.getByRole('menuitem', { name: /^Delete Sandbox/ }));
  await actor.click(
    await screen.findByRole('checkbox', { name: 'I understand this Sandbox will be deleted.' }),
  );
  await actor.click(screen.getByRole('button', { name: 'Delete Sandbox' }));
  await waitFor(() => expect(deleteSandbox).toHaveBeenCalledOnce());
  expect(
    await screen.findByText(
      'Sandbox delete request accepted. Cleanup continues in the background.',
    ),
  ).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: /View Sandbox deletion status/ }));
  expect(await screen.findByRole('dialog', { name: 'Deleting Sandbox' })).toBeInTheDocument();
  expect(screen.queryByRole('dialog', { name: 'Sandbox deleted' })).not.toBeInTheDocument();
  expect(updateSandboxRuntime).not.toHaveBeenCalled();
});

it('submits Delete after an owner Resume is accepted but conversation reopening is still pending', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-11-18T00:00:00Z',
    services: [],
  });
  let sandbox: SandboxDetail = sandboxFixture('paused');
  const reviveSandbox = vi.fn(async () => {
    sandbox = {
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
          state: 'recoverable_pending',
          code: 'lifecycle_wait_exceeded',
          operationId: 'revive-fixture',
          startedAt: '2026-11-18T00:00:00Z',
          resultDueAt: '2026-11-18T00:01:30Z',
          continuing: true,
          actions: ['observe', 'retire'],
          retryAfterSeconds: 5,
        },
      },
    };
  });
  const deleteSandbox = vi.fn(async () => {
    sandbox = sandboxFixture('retiring');
  });
  const client = agentClient({ readSandbox: async () => sandbox, reviveSandbox, deleteSandbox });
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={client}>
      <Console
        user={{
          id: 'owner',
          handle: 'Owner',
          visitor: false,
          capabilities: ['use_own_sandboxes', 'delete_own_sandbox'],
        }}
        route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  const resume = await screen.findByRole('button', { name: 'Resume' });
  await waitFor(() => expect(resume).toBeEnabled());
  await actor.click(resume);
  await screen.findByText('Resume request accepted.');
  expect(reviveSandbox).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Resume' })).toBeDisabled();
  await actor.click(screen.getByRole('button', { name: 'More actions' }));
  const remove = screen.getByRole('menuitem', { name: /^Delete Sandbox/ });
  expect(remove).not.toHaveAttribute('aria-disabled', 'true');
  await actor.click(remove);
  await actor.click(
    await screen.findByRole('checkbox', { name: 'I understand this Sandbox will be deleted.' }),
  );
  await actor.click(screen.getByRole('button', { name: 'Delete Sandbox' }));
  await waitFor(() => expect(deleteSandbox).toHaveBeenCalledOnce());
});
