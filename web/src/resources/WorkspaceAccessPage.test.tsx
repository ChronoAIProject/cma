import { render, screen } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import type { TreePermissions } from '../shell/Tree';
import { agentClient } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { WIRE_WORKSPACE_DETAIL } from '../test/rest';
import { WorkspaceAccessPage } from './WorkspaceAccessPage';

const permissions: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

/** The accessible-read client; `shared` decides how the Workspace is classified. */
function resourceViews(shared: boolean, refused = false) {
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname;
    if (refused)
      return Response.json({ code: 'resource_not_found', detail: 'No access.' }, { status: 404 });
    if (path.includes('/resource-access/'))
      return Response.json({
        capabilities: ['view_details'],
        revalidate_at: null,
        policy_revision: 1,
      });
    if (path.endsWith('/workspaces/wsp_1'))
      return Response.json({
        ...WIRE_WORKSPACE_DETAIL,
        id: 'wsp_1',
        ...(shared
          ? {
              access: 'shared',
              name: 'Shared workspace',
              worktrees: [],
              agents: [],
              worktree_count: 0,
              agent_count: 0,
            }
          : {
              worktrees: WIRE_WORKSPACE_DETAIL.worktrees.map((row) => ({
                ...row,
                workspace_id: 'wsp_1',
              })),
              agents: WIRE_WORKSPACE_DETAIL.agents.map((row) => ({
                ...row,
                workspace_id: 'wsp_1',
              })),
            }),
      });
    throw new Error(`Unexpected resource request ${path}`);
  });
  return createClient({ baseUrl: 'https://fixture.invalid', fetch });
}

function show(views?: ReturnType<typeof resourceViews>) {
  const readWorkspace = vi.fn(async () => workspaceFixture());
  render(
    <WorkspaceAccessPage
      client={agentClient({ resourceViews: views, readWorkspace })}
      id="wsp_1"
      revision={0}
      permissions={permissions}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  return readWorkspace;
}

it('gives the owner their own Workspace page', async () => {
  const readWorkspace = show(resourceViews(false));
  expect(
    await screen.findByRole('button', { name: 'New agent in this Workspace' }),
  ).toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Agents and Worktrees' })).toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Refresh Workspace access' }),
  ).not.toBeInTheDocument();
  expect(readWorkspace).toHaveBeenCalled();
});

it('keeps the shared projection, without owner actions or owner reads, for a shared reader', async () => {
  const readWorkspace = show(resourceViews(true));
  expect(await screen.findByRole('heading', { name: 'Shared workspace' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Refresh Workspace access' })).toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'New agent in this Workspace' }),
  ).not.toBeInTheDocument();
  expect(readWorkspace).not.toHaveBeenCalled();
});

it('lets the owner reads decide without an accessible-read client', async () => {
  show(undefined);
  expect(
    await screen.findByRole('button', { name: 'New agent in this Workspace' }),
  ).toBeInTheDocument();
});

it('shows neither page when the access check is refused', async () => {
  const readWorkspace = show(resourceViews(false, true));
  expect(await screen.findByRole('button', { name: 'Check Workspace access' })).toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'New agent in this Workspace' }),
  ).not.toBeInTheDocument();
  expect(readWorkspace).not.toHaveBeenCalled();
});
