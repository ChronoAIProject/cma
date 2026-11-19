import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createClient } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { SandboxTree } from './SandboxTree';

const row = (id: string, status: string) => ({
  access: 'shared',
  id,
  name: `${status} fixture`,
  status,
  created_at: '2026-11-01T00:00:00Z',
  updated_at: '2026-11-01T00:00:00Z',
  last_activity_at: '2026-11-01T00:00:00Z',
  workspace_count: 0,
  worktree_count: 0,
  agent_count: 0,
});

it.each([true, false])(
  'filters only the authorized page and never scans ahead (shared=%s)',
  async (shared) => {
    const requests: URL[] = [];
    let revoked = false;
    const first = row('sbx_a', 'provisioning');
    const second = row('sbx_b', 'installing');
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async (input) => {
        const url = new URL(String(input));
        if (url.pathname === '/api/v1/sandboxes') {
          requests.push(url);
          if (revoked)
            return Response.json({ code: 'forbidden', detail: 'Access revoked' }, { status: 403 });
          return Response.json(
            url.searchParams.has('cursor')
              ? { items: [second, row('sbx_z', 'retired')] }
              : { items: [first], next: 'sbx_a' },
          );
        }
        if (url.pathname.startsWith('/api/v1/resource-access/'))
          return Response.json({
            capabilities: ['view_details'],
            revalidate_at: null,
            policy_revision: 1,
          });
        const record = url.pathname.endsWith('sbx_a') ? first : second;
        return Response.json({ ...record, workspaces: [] });
      },
    });
    const onNavigate = vi.fn();
    const props = { client, shared, organization: shared ? undefined : 'research', onNavigate };
    const view = render(<SandboxTree {...props} statusFilter="installing" />);
    await screen.findByText('No Sandboxes match Installing on this page.');
    expect(screen.queryByText('No Sandboxes')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next Sandboxes' })).toBeEnabled();
    expect(requests).toHaveLength(1);
    expect(requests[0]?.searchParams.get('shared')).toBe(shared ? 'true' : null);
    expect(requests[0]?.searchParams.get('organization')).toBe(shared ? null : 'research');
    expect(requests[0]?.searchParams.has('status')).toBe(false);
    view.rerender(<SandboxTree {...props} statusFilter="provisioning" />);
    const group = screen.getByRole('region', { name: 'Provisioning sandboxes' });
    expect(
      await within(group).findByRole('treeitem', { name: /provisioning fixture/ }),
    ).toBeVisible();
    expect(group.querySelector('.resource-tree__count')).toHaveTextContent('1');
    expect(requests).toHaveLength(1);
    await userEvent.setup().click(screen.getByRole('button', { name: 'Next Sandboxes' }));
    await screen.findByText('No Sandboxes match Provisioning on this page.');
    expect(requests).toHaveLength(2);
    expect(requests[1]?.searchParams.get('cursor')).toBe('sbx_a');
    view.rerender(<SandboxTree {...props} statusFilter="installing" />);
    expect(await screen.findByRole('treeitem', { name: /installing fixture/ })).toBeVisible();
    expect(requests).toHaveLength(2);
    expect(screen.queryByText('retired fixture')).not.toBeInTheDocument();
    revoked = true;
    await userEvent.setup().click(
      screen.getByRole('button', {
        name: shared ? 'Refresh shared Sandboxes' : 'Refresh organization',
      }),
    );
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.queryByRole('treeitem')).not.toBeInTheDocument();
    expect(onNavigate).not.toHaveBeenCalled();
  },
);

it('names the selected status when the authorized page itself is empty', async () => {
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () => Response.json({ items: [] }),
  });
  render(<SandboxTree client={client} shared statusFilter="provisioning" onNavigate={vi.fn()} />);
  expect(await screen.findByText('No Sandboxes match Provisioning on this page.')).toBeVisible();
});
