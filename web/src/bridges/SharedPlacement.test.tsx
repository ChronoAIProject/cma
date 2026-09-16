import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { BridgePage } from './BridgePage';
import { BridgeManager } from './BridgeManager';
import { chooseOption } from '../test/select';
import { placementCatalog, placementTransport, savedShared } from './placement.test-support';

async function fill(
  sharedWorkspace: boolean,
  organization?: string,
  override?: Parameters<typeof placementTransport>[1],
) {
  const test = placementTransport(organization, override);
  render(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new', organization }}
      onNavigate={vi.fn()}
    />,
  );
  const user = userEvent.setup();
  await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
  await user.type(screen.getByLabelText('Bridge name'), 'Shared research');
  await chooseOption(user, 'Existing channel bot', 'telegram · bot-one');
  await chooseOption(user, 'Existing channel key', 'key-one');
  await user.click(screen.getByRole('checkbox', { name: 'route-one' }));
  await chooseOption(
    user,
    'Resource placement',
    sharedWorkspace
      ? 'A new Agent in an existing Workspace'
      : 'A new Workspace in an existing Sandbox',
  );
  await waitFor(() => expect(screen.getByRole('button', { name: 'Shared Sandbox' })).toBeEnabled());
  await chooseOption(user, 'Shared Sandbox', 'Atlas');
  if (sharedWorkspace) {
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Shared Workspace' })).toBeEnabled(),
    );
    await chooseOption(user, 'Shared Workspace', 'Project');
  } else await chooseOption(user, 'workspace profile', 'Research workspace · Revision 1');
  await chooseOption(user, 'agent profile', 'Research agent · Revision 1');
  await user.click(screen.getByRole('checkbox', { name: /I reviewed who owns/ }));
  return { ...test, user };
}

it.each([false, true])(
  'saves exact shared resources, pins, reserves and explicit trust (Workspace=%s)',
  async (workspace) => {
    const test = await fill(workspace, workspace ? 'org-one' : undefined);
    const save = screen.getByRole('button', { name: 'Save disabled bridge' });
    expect(save).toBeDisabled();
    const trust = screen.getByRole('checkbox', { name: /I acknowledge that all participants/ });
    expect(trust).not.toBeChecked();
    await test.user.click(trust);
    await waitFor(() => expect(save).toBeEnabled());
    const reserve = screen.getByLabelText('Agent slots per Workspace reserved for other work');
    await test.user.clear(reserve);
    await test.user.type(reserve, '0');
    expect(save).toBeDisabled();
    await test.user.clear(reserve);
    await test.user.type(reserve, '2');
    await test.user.click(trust);
    await test.user.click(screen.getByRole('checkbox', { name: /I reviewed who owns/ }));
    await waitFor(() => expect(save).toBeEnabled());
    await test.user.click(save);
    await waitFor(() => expect(test.writes).toHaveLength(1));
    const body = JSON.parse(String(test.writes[0]!.body));
    expect(body.config.placement).toEqual({
      kind: workspace ? 'worktree_agent_per_partition' : 'workspace_per_partition',
      sandbox_id: 'sandbox-1',
      ...(workspace
        ? { workspace_id: 'workspace-1' }
        : { workspace_profile: { id: 'workspace-1', revision: 1 } }),
      agent_profile: { id: 'agent-1', revision: 1 },
      shared_trust: 'all_participants_share_filesystem_processes_and_credentials',
      capacity: { keep_other_workspaces: 1, keep_other_agents_per_workspace: 2 },
    });
    expect(body.owner).toEqual(
      workspace
        ? { kind: 'organization', subject: 'org-one' }
        : { kind: 'person', subject: 'owner' },
    );
    const inventory = test.fetch.mock.calls
      .map(([url]) => new URL(String(url)))
      .find((url) => url.pathname.endsWith('/sandboxes'))!;
    expect(inventory.searchParams.get('organization')).toBe(workspace ? 'org-one' : null);
  },
);

it('rechecks capability availability at save and sends no mutation after it changes', async () => {
  let available = true;
  const test = await fill(false, undefined, (url) =>
    url.pathname.endsWith('/capabilities')
      ? Response.json({
          ...placementCatalog,
          placements: placementCatalog.placements.map((mode) => ({
            ...mode,
            availability: available ? 'available' : 'continuity_unverified',
          })),
        })
      : undefined,
  );
  await test.user.click(screen.getByRole('checkbox', { name: /I acknowledge/ }));
  const save = screen.getByRole('button', { name: 'Save disabled bridge' });
  await waitFor(() => expect(save).toBeEnabled());
  available = false;
  await test.user.click(save);
  await screen.findByRole('alert');
  expect(test.writes).toHaveLength(0);
  expect(screen.queryByRole('button', { name: /Retry create/ })).not.toBeInTheDocument();
});

it('rechecks target authority at save instead of trusting an earlier Available catalog', async () => {
  let revoked = false;
  const test = await fill(true, 'org-one', (url) =>
    url.pathname.includes('/resource-access/') && revoked
      ? Response.json({
          owner: { kind: 'organization', organization_id: 'other' },
          capabilities: ['manage_sandbox'],
          revalidate_at: null,
          policy_revision: 2,
        })
      : undefined,
  );
  await test.user.click(screen.getByRole('checkbox', { name: /I acknowledge/ }));
  const save = screen.getByRole('button', { name: 'Save disabled bridge' });
  await waitFor(() => expect(save).toBeEnabled());
  revoked = true;
  await test.user.click(save);
  await screen.findByRole('alert');
  expect(test.writes).toHaveLength(0);
});

it('recovers an uncertain shared save with its original request despite later unavailability', async () => {
  const attempts: RequestInit[] = [];
  let available = true;
  const test = await fill(false, undefined, (url, init) => {
    if (url.pathname.endsWith('/bridges') && init.method === 'POST') {
      attempts.push(init);
      if (attempts.length === 1) return Promise.reject(new Error('Response lost'));
    }
    if (url.pathname.endsWith('/capabilities') && !available)
      return Response.json({ ...placementCatalog, placements: [] });
    return undefined;
  });
  await test.user.click(screen.getByRole('checkbox', { name: /I acknowledge/ }));
  const save = screen.getByRole('button', { name: 'Save disabled bridge' });
  await waitFor(() => expect(save).toBeEnabled());
  await test.user.click(save);
  const retry = await screen.findByRole('button', { name: 'Recover saved bridge' });
  const reads = test.fetch.mock.calls.filter(([url]) =>
    String(url).includes('/capabilities'),
  ).length;
  available = false;
  await test.user.click(retry);
  await waitFor(() => expect(attempts).toHaveLength(2));
  expect(attempts[1]!.body).toBe(attempts[0]!.body);
  expect(new Headers(attempts[1]!.headers).get('Idempotency-Key')).toBe(
    new Headers(attempts[0]!.headers).get('Idempotency-Key'),
  );
  expect(new Headers(attempts[0]!.headers).get('Idempotency-Key')).toBeTruthy();
  expect(
    test.fetch.mock.calls.filter(([url]) => String(url).includes('/capabilities')),
  ).toHaveLength(reads);
  await waitFor(() =>
    expect(screen.queryByRole('button', { name: 'Recover saved bridge' })).not.toBeInTheDocument(),
  );
});

it.each([undefined, [{ kind: 'workspace_per_partition', availability: 'unknown' }], null])(
  'keeps absent or malformed placement catalogs unavailable: %s',
  async (placements) => {
    const test = placementTransport(undefined, (url) =>
      url.pathname.endsWith('/capabilities')
        ? Response.json({ ...placementCatalog, placements })
        : undefined,
    );
    render(
      <BridgePage
        client={test.client}
        viewerId="owner"
        route={{ kind: 'bridge_new' }}
        onNavigate={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Resource placement' })).toBeEnabled(),
    );
    await userEvent.setup().click(screen.getByRole('button', { name: 'Resource placement' }));
    const option = await screen.findByRole('menuitemradio', {
      name: /A new Workspace in an existing Sandbox/,
    });
    expect(option).toHaveAttribute('aria-disabled', 'true');
  },
);

it('retains unavailable placement settings and leaves existing lifecycle controls usable', async () => {
  const test = placementTransport(undefined, (url) => {
    if (url.pathname.endsWith('/configuration'))
      return Response.json({
        ...savedShared,
        bridge: { ...savedShared.bridge, state: { kind: 'enabled' } },
      });
    if (url.pathname.endsWith('/capabilities'))
      return Response.json({
        ...placementCatalog,
        placements: placementCatalog.placements.map((mode) => ({
          ...mode,
          availability: 'continuity_unverified',
        })),
      });
    return undefined;
  });
  render(<BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />);
  await screen.findByText(/Shared placement has not been enabled by the operator/);
  expect(screen.getByRole('button', { name: 'Shared Sandbox' })).toHaveTextContent('sandbox-1');
  expect(screen.getByRole('button', { name: 'Shared Workspace' })).toHaveTextContent('workspace-1');
  expect(screen.getByRole('button', { name: 'agent profile' })).toHaveTextContent('Revision 7');
  expect(screen.getByLabelText('Workspace slots reserved for other work')).toHaveValue(2);
  expect(screen.getByRole('checkbox', { name: /I acknowledge/ })).toBeChecked();
  expect(screen.getByRole('button', { name: 'Shared Sandbox' })).toBeDisabled();
  expect(screen.getByRole('button', { name: /Disable bridge/ })).toBeEnabled();
});

it('allows an unrelated edit with the exact retained unavailable placement', async () => {
  const updates: RequestInit[] = [];
  const test = placementTransport(undefined, (url, init) => {
    if (url.pathname.endsWith('/configuration') && init.method !== 'PUT')
      return Response.json(savedShared);
    if (url.pathname.endsWith('/capabilities'))
      return Response.json({ ...placementCatalog, placements: [] });
    if (url.pathname.endsWith('/bridges/brg_one/configuration') && init.method === 'PUT') {
      updates.push(init);
      return Response.json({
        bridge_id: 'brg_one',
        owner: savedShared.bridge.owner,
        actor: { kind: 'human', subject: 'owner' },
        revision: 2,
        config_revision: 2,
        command: 'update',
        applied_at: savedShared.bridge.updated_at,
      });
    }
    return undefined;
  });
  render(<BridgeManager client={test.client} id="brg_one" allowed onChanged={vi.fn()} />);
  await screen.findByText(/Shared placement availability could not be verified/);
  const user = userEvent.setup();
  await waitFor(() => expect(screen.getByLabelText('Bridge name')).toBeEnabled());
  await user.type(screen.getByLabelText('Bridge name'), ' updated');
  await user.click(screen.getByRole('checkbox', { name: /I reviewed resource ownership/ }));
  const save = screen.getByRole('button', { name: 'Save settings' });
  await waitFor(() => expect(save).toBeEnabled());
  await user.click(save);
  await waitFor(() => expect(updates).toHaveLength(1));
  expect(JSON.parse(String(updates[0]!.body)).config.placement).toEqual(
    savedShared.config.placement,
  );
  await screen.findByText(/Bridge settings saved/);
});

it('clears a stale Available catalog before an owner-scope response returns', async () => {
  let finish: ((response: Response) => void) | undefined;
  const test = placementTransport(undefined, (url) =>
    url.pathname.endsWith('/capabilities') && url.searchParams.has('organization')
      ? new Promise<Response>((resolve) => {
          finish = resolve;
        })
      : undefined,
  );
  const view = render(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new' }}
      onNavigate={vi.fn()}
    />,
  );
  await screen.findByText(/Text messages are supported/);
  view.rerender(
    <BridgePage
      client={test.client}
      viewerId="owner"
      route={{ kind: 'bridge_new', organization: 'org-one' }}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.queryByText(/Text messages are supported/)).not.toBeInTheDocument();
  await waitFor(() => expect(finish).toBeDefined());
  await act(async () => finish?.(new Response(null, { status: 403 })));
  await screen.findByText(/Connector capabilities are unavailable/);
});
