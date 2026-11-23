import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { Console } from '../Console';
import { agentClient } from '../test/agent';
import { WIRE_SANDBOX_DETAIL } from '../test/rest';
import { chooseOption, expandNavigationFilters } from '../test/select';
import { loadSandbox, loadWorkspace } from './loadHierarchy';
import { ScopedNavigation as ScopedNavigationTree } from './ScopedNavigation';
import { useNavigationScope } from './NavigationScope';
import { useState, type ComponentProps } from 'react';
import type { AppRoute } from '../routing/routes';
import { SandboxAccessPage } from './SandboxAccessPage';
import { agent, at, fixture, sandbox, workspace, worktree } from './SharedHierarchy.fixture';

it('opens a shared Sandbox using safe metadata and loads a Workspace only when expanded', async () => {
  const test = fixture();
  test.show();
  await screen.findByRole('heading', { name: sandbox.name });
  expect(test.paths.some((path) => path.includes('/workspaces/'))).toBe(false);
  expect(screen.queryByRole('button', { name: 'Advanced' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Sharing' })).not.toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: `Show Agents in ${workspace.name}` }));
  const link = await screen.findByRole('link', { name: new RegExp(agent.title) });
  expect(link).toHaveAttribute('href', '/agents/agt_shared');
  expect(screen.getByText(worktree.name)).toBeInTheDocument();
  await user.click(link);
  expect(test.navigate).toHaveBeenCalledWith({ kind: 'agent', agentId: agent.id });
  for (const read of [test.readSandbox, test.readWorkspace, test.readAgent])
    expect(read).not.toHaveBeenCalled();
  expect(
    test.paths.some((path) => /profiles|resources|connections|events|\/usage(?:\?|$)/.test(path)),
  ).toBe(false);
});

it('clears parent and descendant labels after revoked hierarchy access', async () => {
  const test = fixture();
  test.show();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: `Show Agents in ${workspace.name}` }));
  await screen.findByRole('link', { name: new RegExp(agent.title) });
  test.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh Sandbox access' }));
  await screen.findByRole('heading', { name: 'Sandbox unavailable' });
  for (const name of [sandbox.name, workspace.name, worktree.name, agent.title])
    expect(screen.queryByText(name)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Account menu for Reader' })).toBeInTheDocument();
});

it('refuses a direct Sandbox URL without fetching its details or children when no parent grant exists', async () => {
  const test = fixture();
  test.state.revoked = true;
  test.show();
  await screen.findByRole('heading', { name: 'Sandbox unavailable' });
  expect([...new Set(test.paths.filter((path) => !path.endsWith('/organizations')))]).toEqual([
    '/api/v1/resource-access/sandbox/sbx_shared',
  ]);
  expect(test.readSandbox).not.toHaveBeenCalled();
});

it('refuses hierarchy buffered across a policy change and mismatched child assignments', async () => {
  const test = fixture();
  const original = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input) => {
    if (String(input).endsWith('/sandboxes/' + sandbox.id)) test.state.policy += 1;
    return original(input);
  });
  await expect(loadSandbox(test.sdk, sandbox.id, new AbortController().signal)).rejects.toThrow(
    'Resource access changed',
  );
  test.fetch.mockImplementation(async (input) =>
    String(input).endsWith('/workspaces/' + workspace.id)
      ? Response.json({
          ...workspace,
          worktrees: [{ ...worktree, agent_id: 'another_agent' }],
          agents: [agent],
        })
      : original(input),
  );
  await expect(
    loadWorkspace(test.sdk, workspace.id, new AbortController().signal, sandbox.id),
  ).rejects.toThrow('Agent assignment changed');
  await expect(
    loadWorkspace(test.sdk, workspace.id, new AbortController().signal, 'foreign_sandbox'),
  ).rejects.toThrow('hierarchy identity changed');
});

it.each([false, true])(
  'continues an empty scoped page and expands independently authorized children (shared=%s)',
  async (shared) => {
    const test = fixture(shared);
    test.state.emptyFirstPage = true;
    render(
      <ScopedNavigation
        client={test.sdk}
        scope={shared ? 'shared' : 'organization:org_research'}
        onNavigate={test.navigate}
        personal={<p>Personal tree</p>}
      />,
    );
    await screen.findByText(
      shared ? /No shared Sandboxes on this page/ : /No authorized Sandboxes on this page/,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: 'Next Sandboxes' }));
    await screen.findByRole('link', { name: new RegExp(agent.title) });
    if (shared) {
      expect(screen.queryByRole('link', { name: 'All authorized Agents' })).not.toBeInTheDocument();
      expect(test.paths.some((path) => path.includes('/agents/provenance'))).toBe(false);
    } else {
      expect(screen.getByRole('link', { name: 'All authorized Agents' })).toHaveAttribute(
        'href',
        '/organizations/org_research/agents',
      );
    }
    expect(screen.getByRole('button', { name: 'Previous Sandboxes' })).toBeEnabled();
  },
);

it.each([false, true])(
  'cancels a scoped page when returning to personal and discards late responses (shared=%s)',
  async (shared) => {
    const test = fixture(shared);
    const original = test.fetch.getMockImplementation()!;
    let finish: ((response: Response) => void) | undefined;
    test.fetch.mockImplementation(async (input) =>
      new URL(String(input)).pathname === '/api/v1/sandboxes'
        ? new Promise((resolve) => {
            finish = resolve;
          })
        : original(input),
    );
    const props = {
      client: test.sdk,
      onNavigate: test.navigate,
      personal: <p>Personal tree</p>,
    };
    const view = render(
      <ScopedNavigation {...props} scope={shared ? 'shared' : 'organization:org_research'} />,
    );
    await waitFor(() => expect(finish).toBeDefined());
    view.rerender(<ScopedNavigation {...props} scope="" />);
    await act(async () => finish?.(Response.json({ items: [sandbox] })));
    expect(screen.getByText('Personal tree')).toBeInTheDocument();
    expect(screen.queryByText(sandbox.name)).not.toBeInTheDocument();
  },
);

it.each([false, true])(
  'preserves the same personal composer through scope selection and loss (shared=%s)',
  async (shared) => {
    const test = fixture(shared);
    const web = agentClient({ resourceViews: test.sdk });
    render(
      <CmaClientProvider client={web}>
        <Console
          user={{
            id: 'owner',
            handle: 'Owner',
            visitor: false,
            capabilities: ['use_own_sandboxes'],
          }}
          route={{ kind: 'agent', agentId: 'agt_1' }}
          onNavigate={vi.fn()}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
    const user = userEvent.setup();
    const composer = await screen.findByRole('textbox', { name: 'Message' });
    await user.type(composer, 'Keep my personal draft');
    await user.click(screen.getByRole('button', { name: 'Environments' }));
    await expandNavigationFilters(user);
    await chooseOption(user, 'Navigation scope', shared ? 'Shared' : 'Research (Organization)');
    await screen.findByRole('link', { name: new RegExp(sandbox.name) });
    test.state.revoked = true;
    await user.click(
      screen.getByRole('button', {
        name: shared ? 'Refresh shared Sandboxes' : 'Refresh organization',
      }),
    );
    await screen.findByText('Access removed');
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
    expect(composer).toHaveValue('Keep my personal draft');
    await chooseOption(user, 'Navigation scope', 'Personal');
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  },
);

it('never reuses personal Sandbox classification for a different resource', async () => {
  const test = fixture();
  const original = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input) =>
    String(input).endsWith('/sandboxes/sbx_personal')
      ? Response.json({ ...WIRE_SANDBOX_DETAIL, id: 'sbx_personal', workspaces: [] })
      : original(input),
  );
  const view = render(
    <SandboxAccessPage
      client={test.sdk}
      id="sbx_personal"
      personal={<p>Personal controls</p>}
      onNavigate={vi.fn()}
    />,
  );
  await screen.findByText('Personal controls');
  view.rerender(
    <SandboxAccessPage
      client={test.sdk}
      id={sandbox.id}
      personal={<p>Personal controls</p>}
      onNavigate={vi.fn()}
    />,
  );
  expect(screen.queryByText('Personal controls')).not.toBeInTheDocument();
  await screen.findByRole('heading', { name: sandbox.name });
  expect(screen.queryByText('Personal controls')).not.toBeInTheDocument();
});

it('pages shared navigation without parent reads and clears it on revocation', async () => {
  const calls: URL[] = [];
  let revoked = false;
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const url = new URL(String(input));
      calls.push(url);
      if (url.pathname.endsWith('/organizations')) return Response.json({ items: [] });
      if (url.pathname === '/api/v1/sandboxes') {
        expect(url.searchParams.get('shared')).toBe('true');
        return Response.json({ items: [] });
      }
      if (url.pathname === '/api/v1/resource-access/agent/agt_shared')
        return Response.json({
          owner: { kind: 'person', is_current_person: false },
          capabilities: ['view_details'],
          revalidate_at: null,
          policy_revision: 1,
        });
      expect(url.pathname).toBe('/api/v1/agents/provenance');
      expect(url.searchParams.get('shared')).toBe('true');
      if (revoked)
        return Response.json(
          { code: 'resource_not_found', detail: 'Access removed' },
          { status: 404 },
        );
      if (!url.searchParams.has('cursor')) return Response.json({ items: [], next: 'agt_scanned' });
      return Response.json({
        items: [
          {
            agent_id: 'agt_shared',
            title: 'Narrow shared Agent',
            status: 'ready',
            created_at: at,
            updated_at: at,
            creation_origin: 'manual',
          },
        ],
      });
    },
  });
  render(
    <ScopedNavigation
      client={sdk}
      scope="shared"
      view="agent"
      onNavigate={vi.fn()}
      personal={<p>Personal tree</p>}
    />,
  );
  await screen.findByText('No Agents match this scope and status on this page.');
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Next Agents' }));
  await screen.findByRole('link', { name: /Narrow shared Agent/ });
  expect(screen.getByRole('link', { name: 'All authorized Agents' })).toHaveAttribute(
    'href',
    '/shared/agents',
  );
  expect(
    calls
      .filter((url) => url.pathname === '/api/v1/agents/provenance')
      .at(-1)
      ?.searchParams.get('cursor'),
  ).toBe('agt_scanned');
  revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh Agents' }));
  expect(screen.queryByText('Narrow shared Agent')).not.toBeInTheDocument();
  await screen.findByText('Access removed');
});

it('shows an Agent-only share only in Agents view while preserving shared Sandbox children', async () => {
  const test = fixture(true);
  const original = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input) => {
    const url = new URL(String(input));
    if (url.pathname.endsWith('/agents/provenance')) {
      test.paths.push(url.pathname + url.search);
      expect(url.searchParams.get('shared')).toBe('true');
      return Response.json({
        items: [
          {
            agent_id: 'agt_narrow',
            title: 'Agent-only share',
            status: 'ready',
            created_at: at,
            updated_at: at,
            creation_origin: 'manual',
          },
        ],
      });
    }
    return original(input);
  });
  const props = {
    client: test.sdk,
    scope: 'shared',
    onNavigate: test.navigate,
    personal: <p>Personal tree</p>,
  };
  const view = render(<ScopedNavigation {...props} view="sandbox" />);
  await screen.findByRole('link', { name: new RegExp(agent.title) });
  expect(screen.getByRole('link', { name: new RegExp(workspace.name) })).toBeInTheDocument();
  expect(screen.queryByRole('link', { name: /Agent-only share/ })).not.toBeInTheDocument();
  expect(test.paths.some((path) => path.includes('/agents/provenance'))).toBe(false);

  view.rerender(<ScopedNavigation {...props} view="agent" />);
  await screen.findByRole('link', { name: /Agent-only share/ });
  expect(screen.queryByRole('link', { name: new RegExp(workspace.name) })).not.toBeInTheDocument();
  const inventoryReads = test.paths.filter((path) => path.includes('/agents/provenance')).length;
  expect(inventoryReads).toBeGreaterThan(0);

  view.rerender(<ScopedNavigation {...props} view="sandbox" />);
  await screen.findByRole('link', { name: new RegExp(agent.title) });
  expect(screen.queryByRole('link', { name: /Agent-only share/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Next Agents' })).not.toBeInTheDocument();
  expect(test.paths.filter((path) => path.includes('/agents/provenance'))).toHaveLength(
    inventoryReads,
  );
});

function ScopedNavigation(props: Omit<ComponentProps<typeof ScopedNavigationTree>, 'choices'>) {
  const choices = useNavigationScope(props.client, props.scope);
  return <ScopedNavigationTree {...props} choices={choices} />;
}

it('uses the Personal tree rows for organization links, selected state and keyboard navigation', async () => {
  const test = fixture();
  function Navigation() {
    const [route, setRoute] = useState<AppRoute>({ kind: 'agent', agentId: agent.id });
    return (
      <ScopedNavigation
        client={test.sdk}
        scope="organization:org_research"
        route={route}
        personal={<p>Personal tree</p>}
        onNavigate={(next) => {
          test.navigate(next);
          setRoute(next);
        }}
      />
    );
  }
  render(<Navigation />);
  const agentLink = await screen.findByRole('link', { name: new RegExp(agent.title) });
  const tree = screen.getByRole('tree', { name: 'Live sandboxes' });
  const rows = within(tree).getAllByRole('treeitem');
  expect(rows).toHaveLength(3);
  expect(rows.map((row) => row.getAttribute('aria-level'))).toEqual(['1', '2', '3']);
  expect(rows.every((row) => row.classList.contains('tree-row'))).toBe(true);
  expect(agentLink).toHaveAttribute('aria-current', 'page');
  const actor = userEvent.setup();
  const sandboxLink = within(tree).getByRole('link', { name: new RegExp(sandbox.name) });
  expect(sandboxLink).toHaveAttribute('href', '/sandboxes/sbx_shared');
  await actor.click(sandboxLink);
  expect(test.navigate).toHaveBeenLastCalledWith({ kind: 'sandbox', sandboxId: sandbox.id });
  expect(rows[0]).toHaveAttribute('aria-selected', 'true');
  rows[0]!.focus();
  await actor.keyboard('{ArrowDown}{Enter}');
  expect(test.navigate).toHaveBeenLastCalledWith({ kind: 'workspace', workspaceId: workspace.id });
  expect(within(tree).getByRole('link', { name: new RegExp(workspace.name) })).toHaveAttribute(
    'href',
    '/workspaces/wks_shared',
  );
  expect(rows[1]).toHaveAttribute('aria-selected', 'true');
  await actor.click(within(tree).getByRole('button', { name: `Collapse ${workspace.name}` }));
  expect(screen.queryByRole('link', { name: new RegExp(agent.title) })).not.toBeInTheDocument();
  await actor.click(within(tree).getByRole('button', { name: `Expand ${workspace.name}` }));
  await screen.findByRole('link', { name: new RegExp(agent.title) });
  test.state.revoked = true;
  await actor.click(screen.getByRole('button', { name: 'Refresh organization' }));
  await screen.findByText('Access removed');
  for (const name of [sandbox.name, workspace.name, agent.title])
    expect(screen.queryByText(name)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Options for/ })).not.toBeInTheDocument();
});

it('collapses a revoked Workspace after rechecking its parent without retrying the denied branch', async () => {
  const test = fixture();
  const original = test.fetch.getMockImplementation()!;
  let workspaceDenied = false;
  let deniedReads = 0;
  test.fetch.mockImplementation(async (input) => {
    if (
      workspaceDenied &&
      new URL(String(input)).pathname === `/api/v1/resource-access/workspace/${workspace.id}`
    ) {
      deniedReads += 1;
      return Response.json(
        { code: 'resource_not_found', detail: 'Workspace access removed' },
        { status: 404 },
      );
    }
    return original(input);
  });
  render(
    <ScopedNavigation
      client={test.sdk}
      scope="organization:org_research"
      personal={<p>Personal tree</p>}
      onNavigate={test.navigate}
    />,
  );
  await screen.findByRole('link', { name: new RegExp(agent.title) });
  const actor = userEvent.setup();
  await actor.click(screen.getByRole('button', { name: `Collapse ${workspace.name}` }));
  workspaceDenied = true;
  await actor.click(screen.getByRole('button', { name: `Expand ${workspace.name}` }));
  await screen.findByRole('button', { name: `Expand ${workspace.name}` });
  expect(deniedReads).toBe(1);
  expect(screen.queryByRole('link', { name: new RegExp(agent.title) })).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: new RegExp(sandbox.name) })).toBeInTheDocument();
  await actor.click(screen.getByRole('button', { name: 'Refresh organization' }));
  await screen.findByRole('button', { name: `Expand ${workspace.name}` });
  expect(deniedReads).toBe(1);
});
