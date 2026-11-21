import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { SandboxAccessPage } from './SandboxAccessPage';

const at = '2026-09-15T00:00:00Z';
function fixture() {
  const state = {
    capabilities: ['view_details', 'manage_sandbox'],
    revision: 7 as number | undefined,
    status: 'active',
    revoked: false,
    loseResponse: false,
    outcome: 'queued',
    changedPreflight: false,
    reads: 0,
    receiptReads: 0,
  };
  const posts: { key: string | null; body: { command: string; expected_revision: number } }[] = [];
  let deferredReceipt: ((value: Response) => void) | undefined;
  let deferReceipt = false;
  let receiptSignal: AbortSignal | null | undefined;
  const receipt = () => ({
    command_id: 'scmd_one',
    sandbox_id: 'sbx_shared',
    ...posts[0]?.body,
    admitted_at: at,
    queue_deadline: '2026-09-15T00:05:00Z',
    revision: state.outcome === 'queued' ? 1 : 2,
    state:
      state.outcome === 'queued'
        ? { status: 'queued' }
        : state.outcome === 'applied'
          ? { status: 'applied', at, sandbox_revision: 8, disposition: 'scheduled' }
          : { status: 'rejected', at, reason: 'connections_required' },
  });
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const path = new URL(String(input)).pathname;
    if (state.revoked)
      return Response.json(
        { code: 'resource_not_found', detail: 'Sandbox access was removed.' },
        { status: 404 },
      );
    if (path.includes('/resource-access/')) {
      state.reads++;
      if (state.changedPreflight && state.reads >= 3) state.capabilities = ['view_details'];
      return Response.json({
        capabilities: state.capabilities,
        policy_revision: 1,
        revalidate_at: null,
      });
    }
    if (path.endsWith('/sandboxes/sbx_shared'))
      return Response.json({
        access: 'shared',
        id: 'sbx_shared',
        name: 'Research Sandbox',
        status: state.status,
        revision: state.revision,
        created_at: at,
        updated_at: at,
        last_activity_at: at,
        workspace_count: 0,
        worktree_count: 0,
        agent_count: 0,
        workspaces: [],
      });
    if (path.endsWith('/sandboxes/sbx_shared/commands')) {
      posts.push({
        key: new Headers(init?.headers).get('Idempotency-Key'),
        body: JSON.parse(String(init?.body)),
      });
      if (state.loseResponse) {
        state.loseResponse = false;
        throw new TypeError('Response lost after admission');
      }
      return Response.json(receipt(), { status: 202 });
    }
    if (path.endsWith('/sandbox-commands/scmd_one')) {
      state.receiptReads++;
      receiptSignal = init?.signal;
      if (deferReceipt)
        return new Promise<Response>((resolve) => {
          deferredReceipt = resolve;
        });
      return Response.json(receipt());
    }
    throw new Error(`Unexpected request ${path}`);
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const scope = {};
  const page = (commandScope = scope) => (
    <SandboxAccessPage
      client={client}
      id="sbx_shared"
      commandScope={commandScope}
      personal={<textarea aria-label="Personal draft" defaultValue="retained draft" />}
      onNavigate={vi.fn()}
    />
  );
  return {
    state,
    posts,
    page,
    defer: () => {
      deferReceipt = true;
    },
    complete: () => deferredReceipt?.(Response.json(receipt())),
    signal: () => receiptSignal,
  };
}

it.each([
  { capabilities: ['view_details'], revision: 7, status: 'active', button: undefined },
  {
    capabilities: ['view_details', 'manage_sandbox'],
    revision: undefined,
    status: 'active',
    button: undefined,
  },
  {
    capabilities: ['view_details', 'manage_sandbox'],
    revision: 7,
    status: 'active',
    button: 'Pause Sandbox',
  },
  {
    capabilities: ['view_details', 'manage_sandbox'],
    revision: 7,
    status: 'paused',
    button: 'Resume Sandbox',
  },
])(
  'offers shared controls only with current capability, revision and status: $status $button',
  async (setting) => {
    const f = fixture();
    Object.assign(f.state, setting);
    render(f.page());
    await screen.findByRole('heading', { name: 'Research Sandbox' });
    for (const name of ['Pause Sandbox', 'Resume Sandbox', 'Retire Sandbox']) {
      if (name === setting.button) expect(screen.getByRole('button', { name })).toBeEnabled();
      else expect(screen.queryByRole('button', { name })).not.toBeInTheDocument();
    }
    expect(f.posts).toHaveLength(0);
  },
);

it('retains an uncertain intent through refresh and remount, then recovers its terminal receipt', async () => {
  const user = userEvent.setup();
  const f = fixture();
  f.state.loseResponse = true;
  const mounted = render(f.page());
  await user.click(await screen.findByRole('button', { name: 'Pause Sandbox' }));
  await screen.findByText('The result is not confirmed. Retry to recover this request.');
  expect(f.posts).toHaveLength(1);
  expect(f.posts[0]?.key).toBeTruthy();
  f.state.revision = 9;
  f.state.status = 'paused';
  await user.click(screen.getByRole('button', { name: 'Refresh Sandbox access' }));
  await screen.findByRole('button', { name: 'Retry Sandbox request' });
  mounted.unmount();
  render(f.page());
  f.state.outcome = 'applied';
  await user.click(await screen.findByRole('button', { name: 'Retry Sandbox request' }));
  await screen.findByText('Request applied. Sandbox status shows runtime progress.');
  expect(f.posts).toHaveLength(2);
  expect(f.posts[1]).toEqual(f.posts[0]);
  expect(f.posts[1]?.body).toEqual({ command: 'pause', expected_revision: 7 });
  await user.click(screen.getByRole('button', { name: 'Done' }));
  expect(screen.getByRole('button', { name: 'Resume Sandbox' })).toBeEnabled();
});

it('checks authority again at the click boundary and suppresses commands after access loss', async () => {
  const user = userEvent.setup();
  const f = fixture();
  render(f.page());
  await screen.findByRole('button', { name: 'Pause Sandbox' });
  f.state.changedPreflight = true;
  await user.click(screen.getByRole('button', { name: 'Pause Sandbox' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry Sandbox request' })).toBeDisabled(),
  );
  expect(f.posts).toHaveLength(0);
  f.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh Sandbox access' }));
  await screen.findByRole('heading', { name: 'Sandbox unavailable' });
  expect(screen.queryByRole('button', { name: 'Retry Sandbox request' })).not.toBeInTheDocument();
  expect(f.posts).toHaveLength(0);
});

it('polls saved receipts, exposes a typed refusal and permits a fresh deliberate action after review', async () => {
  const user = userEvent.setup();
  const f = fixture();
  render(f.page());
  await user.click(await screen.findByRole('button', { name: 'Pause Sandbox' }));
  await screen.findByText('Request queued. Checking progress…');
  f.state.outcome = 'rejected';
  await screen.findByText(
    'The Sandbox owner needs to reconnect its required services.',
    {},
    { timeout: 4000 },
  );
  expect(f.state.receiptReads).toBe(1);
  expect(f.posts).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Done' }));
  expect(screen.getByRole('button', { name: 'Pause Sandbox' })).toBeEnabled();
});

it('aborts pending receipt reads and ignores late success when shared access is removed', async () => {
  const user = userEvent.setup();
  const f = fixture();
  f.defer();
  render(f.page());
  await user.click(await screen.findByRole('button', { name: 'Pause Sandbox' }));
  await waitFor(() => expect(f.state.receiptReads).toBe(1), { timeout: 4000 });
  f.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh Sandbox access' }));
  await screen.findByRole('heading', { name: 'Sandbox unavailable' });
  expect(f.signal()?.aborted).toBe(true);
  f.state.outcome = 'applied';
  await act(async () => f.complete());
  expect(
    screen.queryByText('Request applied. Sandbox status shows runtime progress.'),
  ).not.toBeInTheDocument();
  expect(f.posts).toHaveLength(1);
});

it('requires a separate retirement confirmation for current sharing managers', async () => {
  const user = userEvent.setup();
  const f = fixture();
  f.state.capabilities.push('manage_sharing');
  render(f.page());
  await user.click(await screen.findByRole('button', { name: 'Retire Sandbox' }));
  expect(f.posts).toHaveLength(0);
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(f.posts).toHaveLength(0);
  await user.click(screen.getByRole('button', { name: 'Retire Sandbox' }));
  await user.click(screen.getByRole('button', { name: 'Confirm retirement' }));
  await screen.findByText('Request queued. Checking progress…');
  expect(f.posts[0]?.body).toEqual({ command: 'retire', expected_revision: 7 });
});

it('does not reuse another viewer console’s pending intent even when the SDK client is reused', async () => {
  const user = userEvent.setup();
  const f = fixture();
  f.state.loseResponse = true;
  const mounted = render(f.page());
  await user.click(await screen.findByRole('button', { name: 'Pause Sandbox' }));
  await screen.findByText('The result is not confirmed. Retry to recover this request.');
  mounted.rerender(f.page({}));
  await screen.findByRole('button', { name: 'Pause Sandbox' });
  expect(screen.queryByRole('button', { name: 'Retry Sandbox request' })).not.toBeInTheDocument();
  expect(f.posts).toHaveLength(1);
});
