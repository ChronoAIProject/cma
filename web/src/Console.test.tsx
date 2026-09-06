import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import type { User } from './auth/types';
import { CmaClientProvider } from './api/context';
import { Console } from './Console';
import { agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';

const OWNER: User = {
  id: 'owner',
  handle: 'Owner',
  visitor: false,
  capabilities: ['create_sandbox', 'use_own_sandboxes', 'delete_own_sandbox'],
};

it('opens an Agent from the tree and keeps the public conversation and Workspace details connected', async () => {
  const onNavigate = vi.fn();
  const actor = userEvent.setup();
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={OWNER}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={onNavigate}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('heading', { name: 'Agent one' })).toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveFocus();
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Show Agent details' }));
  expect(screen.getByText('https://example.test/chrono-bot.git')).toBeInTheDocument();
  const sandbox = screen.getByRole('treeitem', { name: /Atlas/ });
  sandbox.focus();
  await actor.keyboard('{Enter}');
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'sandbox', sandboxId: 'sbx_1' });
});

it('pauses from the Sandbox page and retains its idempotency key across an ambiguous failure', async () => {
  const actor = userEvent.setup();
  const pauseSandbox = vi
    .fn()
    .mockRejectedValueOnce(new Error('Connection lost'))
    .mockResolvedValue(undefined);
  const client = agentClient({
    pauseSandbox,
    readSandbox: vi.fn().mockResolvedValue(sandboxFixture()),
  });
  render(
    <CmaClientProvider client={client}>
      <Console
        user={OWNER}
        route={{ kind: 'sandbox', sandboxId: 'sbx_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await actor.click(await screen.findByRole('button', { name: 'Pause' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Connection lost');
  await actor.click(screen.getByRole('button', { name: 'Pause' }));
  await waitFor(() => expect(pauseSandbox).toHaveBeenCalledTimes(2));
  const first = pauseSandbox.mock.calls[0]!;
  expect(first[0]).toBe('sbx_1');
  expect(first[1].idempotencyKey).not.toBe('');
  expect(pauseSandbox.mock.calls[1]![1].idempotencyKey).toBe(first[1].idempotencyKey);
});

it('hides creation controls for visitors while retaining their trigger-created hierarchy', async () => {
  render(
    <CmaClientProvider client={agentClient()}>
      <Console
        user={{ ...OWNER, visitor: true }}
        route={{ kind: 'home' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
        onCreate={vi.fn()}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByRole('tree', { name: 'Sandboxes' })).toHaveTextContent('Agent one');
  expect(screen.queryByRole('button', { name: 'New agent' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Your agents' })).toBeInTheDocument();
});
