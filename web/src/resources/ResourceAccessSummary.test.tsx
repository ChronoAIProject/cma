import { act, render, screen, waitFor } from '@testing-library/react';
import { createClient, type EffectiveAccess } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { ResourceAccessDetails, ResourceAccessSummary } from './ResourceAccessSummary';
import { useAccessSummaries } from './useAccessSummaries';
import { accessRole } from './accessSummary';
import type { ResourceViewsClient } from './client';

const view: EffectiveAccess = {
  owner: { kind: 'organization', organization_id: 'private-org-id' },
  capabilities: ['view_details', 'view_history', 'view_artifacts'],
  revalidate_at: null,
  policy_revision: 1,
};

it('uses effective permissions for an organization admin with a view-only grant', async () => {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    expect(new URL(String(input)).pathname).toBe('/api/v1/organizations');
    return Response.json({
      items: [
        {
          id: 'private-org-id',
          role: 'admin',
          display_name: 'Research team',
        },
      ],
    });
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  render(<ResourceAccessDetails client={client} access={view} />);
  expect(
    await screen.findByText('Owner: Research team · Your access: View only'),
  ).toBeInTheDocument();
  expect(screen.queryByText(/private-org-id|Resource admin/)).not.toBeInTheDocument();
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(String(fetch.mock.calls[0]?.[0])).toContain('/organizations');
});

it('shows safe personal ownership and only claims management allowed by current capabilities', () => {
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch: vi.fn() });
  const access: EffectiveAccess = { ...view, owner: { kind: 'person', is_current_person: false } };
  const rendered = render(<ResourceAccessDetails client={client} access={access} />);
  expect(screen.getByText('Owner: Personal owner · Your access: View only')).toBeInTheDocument();
  rendered.rerender(
    <ResourceAccessDetails
      client={client}
      access={{
        ...access,
        owner: { kind: 'person', is_current_person: true },
        capabilities: ['view_details', 'manage_sharing'],
      }}
    />,
  );
  expect(screen.getByText('Owner: You · Your access: Owner')).toBeInTheDocument();
  expect(accessRole({ ...access, capabilities: ['view_details', 'interact'] })).toBe(
    'Can interact',
  );
  expect(accessRole({ ...access, capabilities: ['view_details', 'operate_agent'] })).toBe(
    'Agent operator',
  );
  expect(accessRole({ ...access, capabilities: ['view_details', 'manage_sandbox'] })).toBe(
    'Sandbox manager',
  );
});

function Inventory({
  client,
  ids,
  scope = '',
}: {
  client: ResourceViewsClient;
  ids: string[];
  scope?: string;
}) {
  const summaries = useAccessSummaries(client, 'agent', ids, scope);
  return (
    <div>
      {ids.map((id) => (
        <div key={id} data-testid={id}>
          <ResourceAccessSummary value={summaries.get(id)} />
        </div>
      ))}
    </div>
  );
}

it('bounds visible-row policy reads to four and never requests parents or private directories', async () => {
  const pending: (() => void)[] = [];
  let active = 0;
  let peak = 0;
  const paths: string[] = [];
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      paths.push(new URL(String(input)).pathname);
      active += 1;
      peak = Math.max(peak, active);
      await new Promise<void>((resolve) => pending.push(resolve));
      active -= 1;
      return Response.json({ ...view, owner: { kind: 'person', is_current_person: false } });
    },
  });
  render(
    <Inventory client={client} ids={Array.from({ length: 8 }, (_, i) => `agent${String(i)}`)} />,
  );
  await waitFor(() => expect(pending).toHaveLength(4));
  await act(async () => pending.splice(0).forEach((resolve) => resolve()));
  await waitFor(() => expect(pending).toHaveLength(4));
  await act(async () => pending.splice(0).forEach((resolve) => resolve()));
  await waitFor(() =>
    expect(screen.getAllByText('Owner: Personal owner · Your access: View only')).toHaveLength(8),
  );
  expect(peak).toBe(4);
  expect(paths).toHaveLength(8);
  expect(paths.every((path) => path.startsWith('/api/v1/resource-access/agent/'))).toBe(true);
});

it('clears current roles immediately on scope change and after access revocation', async () => {
  let revoked = false;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () =>
      revoked
        ? Response.json({ code: 'resource_not_found', detail: 'Access removed' }, { status: 404 })
        : Response.json({ ...view, owner: { kind: 'person', is_current_person: false } }),
  });
  const rendered = render(<Inventory client={client} ids={['agent']} scope="first" />);
  await screen.findByText('Owner: Personal owner · Your access: View only');
  revoked = true;
  rendered.rerender(<Inventory client={client} ids={['agent']} scope="second" />);
  expect(screen.queryByText(/Personal owner/)).not.toBeInTheDocument();
  await waitFor(() =>
    expect(screen.getByText('Owner: Unavailable · Your access: Unavailable')).toBeInTheDocument(),
  );
});
