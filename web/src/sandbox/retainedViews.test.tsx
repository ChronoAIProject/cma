import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { WIRE_SANDBOX_DETAIL } from '../test/rest';
import { OwnerViewsProvider } from '../resources/retainedOwnerViews';
import { SandboxAccessPage } from '../resources/SandboxAccessPage';
import { SandboxPage } from './SandboxPage';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<Value>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
const sandbox = { ...sandboxFixture(), owner: 'owner' };
const permissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};
function fixture() {
  const access = vi.fn(async () => ({
    capabilities: ['view_details'],
    revalidate_at: null,
    policy_revision: 1,
  }));
  const shared = { value: false };
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (path.includes('/resource-access/')) return Response.json(await access());
      if (path.endsWith('/sandboxes/sbx_1'))
        return Response.json({
          ...WIRE_SANDBOX_DETAIL,
          ...(shared.value ? { access: 'shared', name: 'Shared Sandbox', workspaces: [] } : {}),
          id: 'sbx_1',
          workspaces: (shared.value ? [] : WIRE_SANDBOX_DETAIL.workspaces).map((workspace) => ({
            ...workspace,
            sandbox_id: 'sbx_1',
          })),
        });
      throw new Error(`Unexpected path ${path}`);
    },
  });
  const readSandbox = vi.fn(async () => sandbox);
  const client = agentClient({ resourceViews: sdk, readSandbox });
  const draw = (
    show: boolean,
    owner = 'owner',
    activeClient: CmaWebClient = client,
    id = 'sbx_1',
  ) => (
    <CmaClientProvider client={activeClient}>
      <OwnerViewsProvider viewerId={owner}>
        {show ? (
          <SandboxAccessPage
            key={id}
            client={activeClient.resourceViews}
            id={id}
            onNavigate={vi.fn()}
            personal={(accessUncertain) => (
              <SandboxPage
                client={activeClient}
                sandboxId={id}
                viewerId={owner}
                accessUncertain={accessUncertain}
                revision={0}
                permissions={permissions}
                busy={false}
                onAction={vi.fn()}
                onNavigate={vi.fn()}
              />
            )}
          />
        ) : (
          <p>Other page</p>
        )}
      </OwnerViewsProvider>
    </CmaClientProvider>
  );
  return { access, client, readSandbox, draw, shared };
}

it('restores owner Sandbox and child details immediately but holds commands until both fresh checks finish', async () => {
  const f = fixture();
  const view = render(f.draw(true));
  await screen.findByText('gpt-5');
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  const nextAccess = deferred<Awaited<ReturnType<typeof f.access>>>();
  const nextSandbox = deferred<typeof sandbox>();
  f.access.mockImplementation(() => nextAccess.promise);
  f.readSandbox.mockImplementation(() => nextSandbox.promise);
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'chrono-bot' })).toBeInTheDocument();
  expect(screen.getByText('gpt-5')).toBeInTheDocument();
  expect(screen.queryByText('Loading workspaces')).not.toBeInTheDocument();
  expect(screen.queryByText('Opening Sandbox')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
  expect(screen.getByRole('menuitem', { name: /^Delete Sandbox/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
  await act(async () => nextSandbox.resolve({ ...sandbox, name: 'Fresh Sandbox' }));
  expect(screen.getByRole('heading', { name: 'Fresh Sandbox' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  await act(async () =>
    nextAccess.resolve({ capabilities: ['view_details'], revalidate_at: null, policy_revision: 1 }),
  );
  await waitFor(() => expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled());
});

it('removes retained owner content on access refusal and does not restore it on another visit', async () => {
  const f = fixture();
  const view = render(f.draw(true));
  await screen.findByText('gpt-5');
  const nextAccess = deferred<Awaited<ReturnType<typeof f.access>>>();
  f.access.mockImplementation(() => nextAccess.promise);
  f.readSandbox.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  expect(screen.getByText('gpt-5')).toBeInTheDocument();
  await act(async () => nextAccess.reject(new ApiError(403, 'Access removed')));
  await screen.findByRole('heading', { name: 'Sandbox unavailable' });
  expect(screen.queryByText('gpt-5')).not.toBeInTheDocument();
  f.access.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  expect(screen.getByRole('heading', { name: 'Opening Sandbox' })).toBeInTheDocument();
  expect(screen.queryByText('Atlas')).not.toBeInTheDocument();
});

it.each(['owner', 'client', 'resource'] as const)(
  'does not reuse a Sandbox snapshot after a changed %s context',
  async (changed) => {
    const f = fixture();
    const view = render(f.draw(true));
    await screen.findByText('gpt-5');
    f.access.mockImplementation(() => new Promise(() => {}));
    f.readSandbox.mockImplementation(() => new Promise(() => {}));
    view.rerender(f.draw(false));
    view.rerender(
      f.draw(
        true,
        changed === 'owner' ? 'other-owner' : 'owner',
        changed === 'client' ? { ...f.client } : f.client,
        changed === 'resource' ? 'sbx_other' : 'sbx_1',
      ),
    );
    expect(screen.getByRole('heading', { name: 'Opening Sandbox' })).toBeInTheDocument();
    expect(screen.queryByText('Atlas')).not.toBeInTheDocument();
    expect(screen.queryByText('gpt-5')).not.toBeInTheDocument();
  },
);

it('discards a retired snapshot so revisits cannot revive its old active controls', async () => {
  const f = fixture();
  const view = render(f.draw(true));
  await screen.findByText('gpt-5');
  f.readSandbox.mockResolvedValue({ ...sandbox, status: 'retired' });
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  await screen.findByText(
    'This Sandbox has been deleted. Its runtime is gone; retained history remains read-only.',
  );
  f.access.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  expect(screen.getByRole('heading', { name: 'Opening Sandbox' })).toBeInTheDocument();
});

it('replaces cached personal content with the shared projection and forgets the owner snapshot', async () => {
  const f = fixture();
  const view = render(f.draw(true));
  await screen.findByText('gpt-5');
  f.shared.value = true;
  f.readSandbox.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  await screen.findByRole('heading', { name: 'Shared Sandbox' });
  expect(screen.queryByText('gpt-5')).not.toBeInTheDocument();
  expect(screen.queryByText('Agent access')).not.toBeInTheDocument();
  f.access.mockImplementation(() => new Promise(() => {}));
  view.rerender(f.draw(false));
  view.rerender(f.draw(true));
  expect(screen.getByRole('heading', { name: 'Opening Sandbox' })).toBeInTheDocument();
  expect(screen.queryByText('Atlas')).not.toBeInTheDocument();
});
