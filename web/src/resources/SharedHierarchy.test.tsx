import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { Console } from '../Console';
import { agentClient } from '../test/agent';
import { WIRE_AGENT, WIRE_SANDBOX_DETAIL } from '../test/rest';
import { chooseOption } from '../test/select';
import { loadSandbox, loadWorkspace } from './loadHierarchy';
import { ScopedNavigation } from './ScopedNavigation';
import { SandboxAccessPage } from './SandboxAccessPage';

const at = '2026-09-01T00:00:00Z';
const sandbox = {
  access: 'shared',
  id: 'sbx_shared',
  name: 'Research Sandbox',
  status: 'active',
  created_at: at,
  updated_at: at,
  last_activity_at: at,
  workspace_count: 1,
  worktree_count: 1,
  agent_count: 1,
};
const workspace = {
  access: 'shared',
  id: 'wks_shared',
  sandbox_id: sandbox.id,
  name: 'Review Workspace',
  status: 'ready',
  created_at: at,
  worktree_count: 1,
  agent_count: 1,
};
const worktree = {
  access: 'shared',
  id: 'wt_shared',
  sandbox_id: sandbox.id,
  workspace_id: workspace.id,
  name: 'Review branch',
  status: 'ready',
  agent_id: 'agt_shared',
};
const agent = {
  id: 'agt_shared',
  worktree_id: worktree.id,
  sandbox_id: sandbox.id,
  workspace_id: workspace.id,
  status: 'ready',
  title: 'Review conversation',
  updated_at: at,
};

function fixture() {
  const state = { revoked: false, organizations: true, policy: 1, emptyFirstPage: false };
  const paths: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL): Promise<Response> => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    if (url.pathname.endsWith('/organizations'))
      return state.organizations
        ? Response.json({
            items: [{ id: 'org_research', role: 'viewer', display_name: 'Research' }],
          })
        : Response.json(
            { code: 'organization_authority_unavailable', detail: 'Unavailable' },
            { status: 503 },
          );
    if (url.pathname.endsWith('/agents/agt_1'))
      return Response.json({ ...WIRE_AGENT, id: 'agt_1' });
    if (state.revoked)
      return Response.json(
        { code: 'resource_not_found', detail: 'Access removed' },
        { status: 404 },
      );
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        capabilities: ['view_details'],
        revalidate_at: null,
        policy_revision: state.policy,
      });
    if (url.pathname === '/api/v1/sandboxes') {
      expect(url.searchParams.get('organization')).toBe('org_research');
      return Response.json(
        state.emptyFirstPage && !url.searchParams.has('cursor')
          ? { items: [], next: 'sbx_previous' }
          : { items: [sandbox] },
      );
    }
    if (url.pathname.endsWith('/sandboxes/' + sandbox.id))
      return Response.json({ ...sandbox, workspaces: [workspace] });
    if (url.pathname.endsWith('/workspaces/' + workspace.id))
      return Response.json({ ...workspace, worktrees: [worktree], agents: [agent] });
    throw new Error(`Unexpected hierarchy request ${url.pathname}`);
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const readSandbox = vi.fn();
  const readWorkspace = vi.fn();
  const readAgent = vi.fn();
  const web = agentClient({
    resourceViews: sdk,
    listSandboxes: async () => ({ items: [] }),
    readSandbox,
    readWorkspace,
    readAgent,
  });
  const navigate = vi.fn();
  const show = () =>
    render(
      <CmaClientProvider client={web}>
        <Console
          user={{
            id: 'reader',
            handle: 'Reader',
            visitor: false,
            capabilities: ['use_own_sandboxes'],
          }}
          route={{ kind: 'sandbox', sandboxId: sandbox.id }}
          onNavigate={navigate}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
  return { state, sdk, fetch, paths, show, web, navigate, readSandbox, readWorkspace, readAgent };
}

it('opens a shared Sandbox using safe metadata and loads a Workspace only when expanded', async () => {
  const test = fixture();
  test.show();
  await screen.findByRole('heading', { name: sandbox.name });
  expect(test.paths.some((path) => path.includes('/workspaces/'))).toBe(false);
  expect(screen.queryByRole('button', { name: 'Advanced' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Sharing' })).not.toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: workspace.name }));
  const link = await screen.findByRole('link', { name: agent.title });
  expect(link).toHaveAttribute('href', '/agents/agt_shared');
  expect(screen.getByText(worktree.name)).toBeInTheDocument();
  await user.click(link);
  expect(test.navigate).toHaveBeenCalledWith({ kind: 'agent', agentId: agent.id });
  for (const read of [test.readSandbox, test.readWorkspace, test.readAgent])
    expect(read).not.toHaveBeenCalled();
  expect(test.paths.some((path) => /profiles|resources|connections|events|usage/.test(path))).toBe(
    false,
  );
});

it('clears parent and descendant labels after revoked hierarchy access', async () => {
  const test = fixture();
  test.show();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: workspace.name }));
  await screen.findByRole('link', { name: agent.title });
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
  expect(test.paths.filter((path) => !path.endsWith('/organizations'))).toEqual([
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

it('continues an empty organization page and expands only independently authorized children', async () => {
  const test = fixture();
  test.state.emptyFirstPage = true;
  render(
    <ScopedNavigation
      client={test.sdk}
      organization="org_research"
      onScopeChange={vi.fn()}
      onNavigate={test.navigate}
      personal={<p>Personal tree</p>}
    />,
  );
  await screen.findByText(/No authorized Sandboxes on this page/);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Next Sandboxes' }));
  await user.click(await screen.findByRole('button', { name: sandbox.name }));
  expect(test.paths.some((path) => path.includes('/workspaces/'))).toBe(false);
  await user.click(await screen.findByRole('button', { name: workspace.name }));
  await screen.findByRole('link', { name: agent.title });
  expect(screen.getByRole('link', { name: 'All authorized Agents' })).toHaveAttribute(
    'href',
    '/organizations/org_research/agents',
  );
  expect(screen.getByRole('button', { name: 'Previous Sandboxes' })).toBeEnabled();
});

it('cancels an organization page when returning to personal and discards the late response', async () => {
  const test = fixture();
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
    onScopeChange: vi.fn(),
    onNavigate: test.navigate,
    personal: <p>Personal tree</p>,
  };
  const view = render(<ScopedNavigation {...props} organization="org_research" />);
  await waitFor(() => expect(finish).toBeDefined());
  view.rerender(<ScopedNavigation {...props} organization="" />);
  await act(async () => finish?.(Response.json({ items: [sandbox] })));
  expect(screen.getByText('Personal tree')).toBeInTheDocument();
  expect(screen.queryByText(sandbox.name)).not.toBeInTheDocument();
});

it('preserves the same personal composer through organization scope selection and loss', async () => {
  const test = fixture();
  const web = agentClient({ resourceViews: test.sdk });
  render(
    <CmaClientProvider client={web}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  const user = userEvent.setup();
  const composer = await screen.findByRole('textbox', { name: 'Message' });
  await user.type(composer, 'Keep my personal draft');
  await chooseOption(user, 'Navigation scope', 'Research');
  await screen.findByRole('button', { name: sandbox.name });
  test.state.revoked = true;
  await user.click(screen.getByRole('button', { name: 'Refresh organization' }));
  await screen.findByText('Access removed');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep my personal draft');
  await chooseOption(user, 'Navigation scope', 'Personal');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
});

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
