import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { CmaWebClient } from './api/client';
import type { SandboxDetail } from './api/types';
import { Console } from './Console';
import { connectionClient, type ConnectionReport } from './connections/client';
import { agentClient } from './test/agent';
import { parkedRuntimeUpdateFixture, sandboxFixture } from './test/hierarchy';

afterEach(() => vi.restoreAllMocks());

function renderOwnedSandbox(client: CmaWebClient) {
  const onNavigate = vi.fn();
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
        onNavigate={onNavigate}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  return onNavigate;
}

const readyReport: ConnectionReport = {
  owner: 'owner',
  ready: true,
  checked_at: '2026-11-21T14:05:16Z',
  services: [],
};

const missingReport: ConnectionReport = {
  ...readyReport,
  ready: false,
  services: [
    {
      service: 'git-host',
      display_name: 'Git hosting',
      kind: 'nyxid',
      state: 'missing',
      connection_mode: 'hosted',
    },
  ],
};

function refusedResume(sandbox: SandboxDetail): SandboxDetail {
  return {
    ...sandbox,
    revision: sandbox.revision + 1,
    lifecycle: {
      pause: { allowed: false, reason: 'not_active' },
      revive: { allowed: false, reason: 'work_in_progress' },
    },
  };
}

it('offers an eligible owner Resume before checking connections and verifies them on click', async () => {
  let finish!: (report: ConnectionReport) => void;
  const readiness = new Promise<ConnectionReport>((resolve) => {
    finish = resolve;
  });
  const check = vi.spyOn(connectionClient, 'check').mockReturnValue(readiness);
  const sandbox = { ...sandboxFixture('paused'), owner: 'owner' };
  const reviveSandbox = vi.fn().mockResolvedValue(undefined);
  const client = agentClient({ readSandbox: async () => sandbox, reviveSandbox });
  const actor = userEvent.setup();
  renderOwnedSandbox(client);
  const resume = await screen.findByRole('button', { name: 'Resume' });
  expect(check).not.toHaveBeenCalled();
  expect(resume).toBeEnabled();
  expect(reviveSandbox).not.toHaveBeenCalled();

  await actor.click(resume);
  expect(resume).toBeDisabled();
  await actor.click(resume);
  expect(check).toHaveBeenCalledExactlyOnceWith(
    { type: 'sandbox', id: sandbox.id },
    undefined,
    expect.any(AbortSignal),
  );
  expect(reviveSandbox).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await act(async () => finish(readyReport));
  expect(reviveSandbox).toHaveBeenCalledExactlyOnceWith(sandbox.id, {
    idempotencyKey: expect.any(String),
  });
});

it('lets an owner cancel the Resume connection dialog without navigating or resuming', async () => {
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(missingReport);
  const reviveSandbox = vi.fn();
  const actor = userEvent.setup();
  const onNavigate = renderOwnedSandbox(
    agentClient({
      readSandbox: async () => ({ ...sandboxFixture('paused'), owner: 'owner' }),
      reviveSandbox,
    }),
  );
  const resume = await screen.findByRole('button', { name: 'Resume' });
  expect(check).not.toHaveBeenCalled();
  await actor.click(resume);
  const dialog = await screen.findByRole('dialog', { name: 'Connect your accounts' });
  expect(check).toHaveBeenCalledTimes(2);
  expect(resume).toBeDisabled();
  await actor.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(resume).toBeEnabled();
  expect(onNavigate).not.toHaveBeenCalled();
  expect(reviveSandbox).not.toHaveBeenCalled();
});

it.each(['held check', 'open connection dialog'] as const)(
  'withdraws Resume when a newer Sandbox record refuses it during a %s',
  async (pending) => {
    const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(missingReport);
    let finish!: (report: ConnectionReport) => void;
    check.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const sandbox = { ...sandboxFixture('paused'), owner: 'owner' };
    const readSandbox = vi.fn().mockResolvedValue(sandbox);
    const reviveSandbox = vi.fn();
    const actor = userEvent.setup();
    const onNavigate = renderOwnedSandbox(agentClient({ readSandbox, reviveSandbox }));
    const resume = await screen.findByRole('button', { name: 'Resume' });

    await actor.click(resume);
    expect(check).toHaveBeenCalledTimes(1);
    const signal = check.mock.calls[0]![2];
    if (pending === 'open connection dialog') {
      await act(async () => finish(missingReport));
      expect(await screen.findByRole('dialog', { name: 'Connect your accounts' })).toBeVisible();
      expect(check).toHaveBeenCalledTimes(2);
    }

    readSandbox.mockResolvedValue(refusedResume(sandbox));
    await act(async () => fireEvent(document, new Event('visibilitychange')));
    expect(await screen.findByText(/Sandbox work is in progress/)).toBeVisible();
    expect(resume).toBeDisabled();
    if (pending === 'held check') await act(async () => finish(readyReport));
    expect.soft(reviveSandbox).not.toHaveBeenCalled();
    expect.soft(screen.queryByText('Connect your accounts')).not.toBeInTheDocument();
    expect.soft(screen.queryAllByRole('dialog')).toHaveLength(0);
    expect.soft(check).toHaveBeenCalledTimes(pending === 'held check' ? 1 : 2);
    expect.soft(signal?.aborted).toBe(true);
    expect(onNavigate).not.toHaveBeenCalled();
  },
);

it('holds an eligible owner Resume after a failed Sandbox refresh', async () => {
  const check = vi.spyOn(connectionClient, 'check').mockResolvedValue(readyReport);
  const reviveSandbox = vi.fn();
  const readSandbox = vi.fn().mockResolvedValue({ ...sandboxFixture('paused'), owner: 'owner' });
  renderOwnedSandbox(agentClient({ readSandbox, reviveSandbox }));
  const resume = await screen.findByRole('button', { name: 'Resume' });
  expect(resume).toBeEnabled();
  readSandbox.mockRejectedValue(new Error('Sandbox refresh unavailable.'));
  await act(async () => fireEvent(document, new Event('visibilitychange')));
  await waitFor(() => expect(resume).toBeDisabled());
  fireEvent.click(resume);
  expect(check).not.toHaveBeenCalled();
  expect(reviveSandbox).not.toHaveBeenCalled();
});

it('keeps Resume disabled when the server refuses it for the owner', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue(readyReport);
  const reviveSandbox = vi.fn();
  const sandbox = refusedResume({ ...sandboxFixture('paused'), owner: 'owner' });
  renderOwnedSandbox(agentClient({ readSandbox: async () => sandbox, reviveSandbox }));
  const resume = await screen.findByRole('button', { name: 'Resume' });
  expect(resume).toBeDisabled();
  expect(screen.getByText(/Sandbox work is in progress/)).toBeVisible();
  fireEvent.click(resume);
  expect(reviveSandbox).not.toHaveBeenCalled();
});

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
  await actor.click(screen.getByRole('button', { name: 'Request deletion' }));
  await waitFor(() => expect(deleteSandbox).toHaveBeenCalledOnce());
  expect(await screen.findByText('Deleting 1 environment')).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'View status' }));
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
  await actor.click(screen.getByRole('button', { name: 'Request deletion' }));
  await waitFor(() => expect(deleteSandbox).toHaveBeenCalledOnce());
});
