import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { ManagerNavigation } from './ManagerNavigation';
import { navigationFixture } from '../test/hierarchy';
import { chooseOption, expandNavigationFilters } from '../test/select';

const items = ['provisioning', 'installing', 'retiring', 'retired'].map((status) => {
  const item = navigationFixture()[0]!;
  return {
    ...item,
    sandbox: {
      ...item.sandbox,
      id: status,
      name: `${status} fixture`,
      status: status as typeof item.sandbox.status,
    },
    workspaces: [],
  };
});
const props = {
  scope: '',
  route: { kind: 'agent_manager' } as const,
  viewerId: 'filter-viewer',
  permissions: {
    createWorkspace: false,
    createAgent: false,
    pauseSandbox: false,
    deleteSandbox: false,
    removeWorkspace: false,
    stopAgent: false,
    deleteAgent: false,
    exportHistory: false,
  },
  pending: false,
  busySandboxIds: new Set<string>(),
  deletingSandboxIds: new Set<string>(),
  onScope: vi.fn(),
  onViewDeletion: vi.fn(),
  onNavigate: vi.fn(),
  onAction: vi.fn(),
  navigation: { status: 'ready' as const, items, refresh: vi.fn() },
};
async function environments(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: 'Environments' }));
  await expandNavigationFilters(user);
}
afterEach(() => vi.restoreAllMocks());

it.each(['Provisioning', 'Installing'])(
  'aligns %s rows, count and empty state without changing deletion progress',
  async (label) => {
    const callbacks = { onAction: vi.fn(), onNavigate: vi.fn() };
    const view = render(<ManagerNavigation {...props} {...callbacks} />);
    const user = userEvent.setup();
    await environments(user);
    await chooseOption(user, 'Sandbox status filter', label);
    expect(screen.getByRole('button', { name: 'Filter' })).toHaveAccessibleDescription(label);
    const group = screen.getByRole('region', { name: `${label} sandboxes` });
    expect(group.querySelector('.resource-tree__count')).toHaveTextContent('1');
    expect(within(group).getAllByRole('treeitem')).toHaveLength(1);
    expect(within(group).getByRole('treeitem')).toHaveTextContent(`${label.toLowerCase()} fixture`);
    expect(within(group).getByRole('treeitem').querySelector('.tree-row__state')).toHaveTextContent(
      label,
    );
    const tree = within(group.closest<HTMLElement>('.manager-navigation')!);
    const footer = tree.getByText('Cleaning up 1 sandboxes');
    expect(footer.closest('.manager-navigation__list')).toBeNull();
    await user.click(footer);
    expect(tree.getByRole('button', { name: 'View status for retiring fixture' })).toBeVisible();
    expect(screen.queryByText('retired fixture')).not.toBeInTheDocument();
    view.rerender(
      <ManagerNavigation
        {...props}
        {...callbacks}
        navigation={{
          ...props.navigation,
          items: items.filter((item) => item.sandbox.status !== label.toLowerCase()),
        }}
      />,
    );
    expect(within(group).queryByRole('treeitem')).not.toBeInTheDocument();
    expect(group.querySelector('.resource-tree__count')).toHaveTextContent('0');
    expect(screen.getByText(`No Sandboxes match ${label}.`)).toBeVisible();
    expect(screen.queryByText('No Sandboxes')).not.toBeInTheDocument();
    expect(callbacks.onAction).not.toHaveBeenCalled();
    expect(callbacks.onNavigate).not.toHaveBeenCalled();
  },
);

it('restores each view after returning, remounting and clearing without opening environments automatically', async () => {
  const user = userEvent.setup();
  let view = render(<ManagerNavigation {...props} />);
  await expandNavigationFilters(user);
  await chooseOption(user, 'Agent status filter', 'Working');
  await environments(user);
  await chooseOption(user, 'Sandbox status filter', 'Installing');
  await user.click(screen.getByRole('button', { name: 'Tasks' }));
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent('Working');
  await environments(user);
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'Installing',
  );
  view.unmount();
  view = render(<ManagerNavigation {...props} />);
  expect(screen.getByRole('button', { name: 'Tasks' })).toHaveAttribute('aria-pressed', 'true');
  await expandNavigationFilters(user);
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent('Working');
  await environments(user);
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'Installing',
  );
  await user.click(screen.getByRole('button', { name: 'Clear filters' }));
  view.unmount();
  render(<ManagerNavigation {...props} />);
  await expandNavigationFilters(user);
  expect(screen.getByRole('button', { name: 'Agent status filter' })).toHaveTextContent(
    'All statuses',
  );
  await environments(user);
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'All statuses',
  );
});

it('keeps scope and viewer filters separate and resets the environment view on a viewer change', async () => {
  const user = userEvent.setup();
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) =>
      Response.json({
        items: new URL(String(input)).pathname.endsWith('/organizations')
          ? [{ id: 'org_research', role: 'viewer', display_name: 'Research' }]
          : [],
      }),
  });
  const view = render(<ManagerNavigation {...props} client={client} />);
  await environments(user);
  await chooseOption(user, 'Sandbox status filter', 'Installing');
  await chooseOption(user, 'Navigation scope', 'Shared');
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'All statuses',
  );
  await chooseOption(user, 'Sandbox status filter', 'Provisioning');
  await chooseOption(user, 'Navigation scope', 'Research (Organization)');
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'All statuses',
  );
  await chooseOption(user, 'Navigation scope', 'Shared');
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'Provisioning',
  );
  await user.click(screen.getByRole('button', { name: 'Tasks' }));
  await environments(user);
  expect(screen.getByRole('button', { name: 'Navigation scope' })).toHaveTextContent('Shared');
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'Provisioning',
  );
  await chooseOption(user, 'Navigation scope', 'Personal');
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'Installing',
  );
  await chooseOption(user, 'Navigation scope', 'Shared');
  view.rerender(<ManagerNavigation {...props} client={client} viewerId="another-viewer" />);
  expect(screen.getByRole('button', { name: 'Tasks' })).toHaveAttribute('aria-pressed', 'true');
  await environments(user);
  expect(screen.getByRole('button', { name: 'Navigation scope' })).toHaveTextContent('Personal');
  expect(screen.getByRole('button', { name: 'Sandbox status filter' })).toHaveTextContent(
    'All statuses',
  );
});

it('shows a storage failure notice and still filters immediately', async () => {
  vi.spyOn(Object.getPrototypeOf(window.sessionStorage) as Storage, 'setItem').mockImplementation(
    () => {
      throw new Error('Quota');
    },
  );
  render(<ManagerNavigation {...props} />);
  const user = userEvent.setup();
  await environments(user);
  await chooseOption(user, 'Sandbox status filter', 'Installing');
  expect(screen.getByRole('status')).toHaveTextContent(
    'Filters apply now but cannot be remembered after refresh.',
  );
  expect(
    screen
      .getByRole('tree', { name: 'Installing sandboxes' })
      .querySelectorAll('[role="treeitem"]'),
  ).toHaveLength(1);
});

it('discards a late shared inventory response from the previous viewer', async () => {
  let complete!: (value: Response) => void;
  const firstClient = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: vi.fn((input) =>
      new URL(String(input)).pathname === '/api/v1/sandboxes'
        ? new Promise<Response>((resolve) => {
            complete = resolve;
          })
        : Promise.resolve(Response.json({ items: [] })),
    ),
  });
  const secondClient = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: vi.fn(async () => Response.json({ items: [] })),
  });
  const view = render(<ManagerNavigation {...props} client={firstClient} scope="shared" />);
  const user = userEvent.setup();
  await environments(user);
  await chooseOption(user, 'Navigation scope', 'Shared');
  const finish = complete;
  view.rerender(
    <ManagerNavigation {...props} viewerId="another-viewer" client={secondClient} scope="shared" />,
  );
  await environments(user);
  expect(screen.getByRole('button', { name: 'Navigation scope' })).toHaveTextContent('Personal');
  await chooseOption(user, 'Navigation scope', 'Shared');
  await screen.findByText(/No shared Sandboxes on this page/);
  await act(async () =>
    finish(
      Response.json({
        items: [
          {
            access: 'shared',
            created_at: '2026-11-01T00:00:00Z',
            updated_at: '2026-11-01T00:00:00Z',
            last_activity_at: '2026-11-01T00:00:00Z',
            id: 'sbx_old',
            name: 'Previous viewer resource',
            status: 'installing',
            agent_count: 0,
            workspace_count: 0,
            worktree_count: 0,
          },
        ],
      }),
    ),
  );
  expect(screen.queryByText('Previous viewer resource')).not.toBeInTheDocument();
  expect(screen.getByText(/No shared Sandboxes on this page/)).toBeVisible();
});
