import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient, type ResourceShare } from '@chronoai/cma-sdk';
import { SharingButton } from './SharingButton';

const sandbox = { kind: 'sandbox', id: 'sbx_own', name: 'Research Sandbox' } as const;
const at = '2026-09-01T00:00:00Z';
const actor = { kind: 'human', subject: 'owner' } as const;

function fixture() {
  const state = {
    allowed: true,
    organizations: true,
    policy: 1,
    uncertain: false,
    shares: [] as ResourceShare[],
  };
  const writes: { method: string; path: string; key: string | null; body: unknown }[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(String(input));
    const path = url.pathname;
    if (path.endsWith('/organizations'))
      return state.organizations
        ? Response.json({
            items: [{ id: 'org_research', display_name: 'Research', role: 'admin' }],
          })
        : Response.json({ code: 'organization_authority_unavailable' }, { status: 503 });
    if (path.includes('/resource-access/'))
      return Response.json({
        capabilities: state.allowed ? ['view_details', 'manage_sharing'] : ['view_details'],
        revalidate_at: null,
        policy_revision: state.policy,
      });
    if (!path.includes('/resource-shares/')) throw new Error(`Unexpected resource request ${path}`);
    if (!init?.method || init.method === 'GET')
      return Response.json({ items: state.shares.filter((share) => share.revoked_at === null) });
    const body = init.body ? JSON.parse(String(init.body)) : undefined;
    writes.push({
      method: init.method,
      path,
      key: new Headers(init.headers).get('idempotency-key'),
      body,
    });
    if (state.uncertain) {
      state.uncertain = false;
      return Response.json({ code: 'resource_access_unavailable' }, { status: 503 });
    }
    let share: ResourceShare;
    if (init.method === 'POST') {
      share = {
        id: `share_${state.policy}`,
        owner: { kind: 'person', subject: 'owner' },
        selector: { kind: 'sandbox', sandbox_id: sandbox.id },
        target_org_id: 'org_research',
        mode: body.mode,
        issuer_actor: actor,
        created_at: at,
        revoked_at: null,
        revision: 1,
      };
      state.shares.push(share);
    } else {
      const found = state.shares.find((value) => path.endsWith(`/${value.id}`));
      if (found === undefined)
        return Response.json({ code: 'resource_access_changed' }, { status: 409 });
      share = { ...found, revision: found.revision + 1, revoked_at: at };
      state.shares = state.shares.map((value) => (value === found ? share : value));
    }
    state.policy += 1;
    return Response.json({
      operation_id: `operation_${state.policy}`,
      share,
      actor,
      at,
      policy_revision: state.policy,
    });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const show = () => render(<SharingButton client={client} target={sandbox} />);
  const open = async () => {
    const view = show();
    await userEvent.setup().click(await screen.findByRole('button', { name: 'Sharing' }));
    await screen.findByRole('heading', { name: 'Current grants' });
    return view;
  };
  return { state, fetch, writes, open, show };
}

it('does not expose sharing controls or request grants for a recipient', async () => {
  const test = fixture();
  test.state.allowed = false;
  test.show();
  await waitFor(() => expect(test.fetch).toHaveBeenCalledOnce());
  expect(screen.queryByRole('button', { name: 'Sharing' })).not.toBeInTheDocument();
  expect(test.fetch.mock.calls.every(([url]) => String(url).includes('/resource-access/'))).toBe(
    true,
  );
});

it('always shares the whole Sandbox and requires explicit acknowledgement', async () => {
  const test = fixture();
  await test.open();
  expect(screen.getByText('Sharing applies to the whole Sandbox.')).toBeInTheDocument();
  expect(
    screen.getByText(/This scope includes every current and future Workspace and Agent/),
  ).toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Share with organization' }));
  await user.click(screen.getByRole('menuitemradio', { name: 'Research' }));
  expect(screen.getByRole('button', { name: 'Create share' })).toBeDisabled();
  await user.click(screen.getByRole('checkbox', { name: /share the whole Sandbox/i }));
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByText('Current access: View only');
  expect(test.writes[0]).toEqual(
    expect.objectContaining({
      method: 'POST',
      path: '/api/v1/resource-shares/sandbox/sbx_own',
      body: { target_org_id: 'org_research', mode: 'view' },
    }),
  );
  expect(test.writes[0]?.key).toBeTruthy();
});

it('revokes a Sandbox grant with its current revision during an organization outage', async () => {
  const test = fixture();
  test.state.shares = [
    {
      id: 'share_1',
      owner: { kind: 'person', subject: 'owner' },
      selector: { kind: 'sandbox', sandbox_id: sandbox.id },
      target_org_id: 'org_research',
      mode: 'view',
      issuer_actor: actor,
      created_at: at,
      revoked_at: null,
      revision: 1,
    },
  ];
  test.state.organizations = false;
  await test.open();
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Existing grants can still be revoked',
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Revoke' }));
  await user.click(screen.getByRole('button', { name: 'Revoke grant' }));
  await screen.findByText('No direct organization grants.');
  expect(test.writes).toEqual([expect.objectContaining({ method: 'DELETE' })]);
});

it('retains the idempotency key when retrying an unconfirmed create', async () => {
  const test = fixture();
  await test.open();
  test.state.uncertain = true;
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Share with organization' }));
  await user.click(screen.getByRole('menuitemradio', { name: 'Research' }));
  await user.click(screen.getByRole('checkbox', { name: /share the whole Sandbox/i }));
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: 'Create share' }));
  await screen.findByText('Current access: View only');
  expect(test.writes).toHaveLength(2);
  expect(test.writes[1]?.key).toBe(test.writes[0]?.key);
});
