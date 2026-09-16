/** Public shared shapes; absent parent profile/service fields stay absent. */
export const organizationTime = '2026-09-06T00:00:00Z';
export const organizationTarget = {
  sandbox_id: 'sandbox-1',
  workspace_id: 'workspace-1',
  worktree_id: 'worktree-1',
  agent_id: 'agent-1',
};
export const organizationAccess = {
  owner: { kind: 'organization', organization_id: 'org-one' },
  capabilities: ['view_details', 'manage_sandbox', 'manage_bridge', 'interact'],
  revalidate_at: null,
  policy_revision: 1,
};
export const organizationWorkspaceSummary = {
  access: 'shared',
  id: 'workspace-1',
  sandbox_id: 'sandbox-1',
  name: 'Organization project',
  status: 'ready',
  worktree_count: 1,
  agent_count: 1,
};
export const organizationSandbox = {
  access: 'shared',
  id: 'sandbox-1',
  name: 'Organization Sandbox',
  status: 'active',
  revision: 1,
  created_at: organizationTime,
  updated_at: organizationTime,
  last_activity_at: organizationTime,
  workspace_count: 1,
  worktree_count: 1,
  agent_count: 1,
  workspaces: [organizationWorkspaceSummary],
};
export const organizationWorkspace = {
  ...organizationWorkspaceSummary,
  created_at: organizationTime,
  worktrees: [
    {
      access: 'shared',
      id: 'worktree-1',
      workspace_id: 'workspace-1',
      sandbox_id: 'sandbox-1',
      name: 'Organization tree',
      status: 'ready',
      agent_id: 'agent-1',
    },
  ],
  agents: [
    {
      id: 'agent-1',
      sandbox_id: 'sandbox-1',
      workspace_id: 'workspace-1',
      worktree_id: 'worktree-1',
      status: 'ready',
      title: 'Organization Agent',
      updated_at: organizationTime,
    },
  ],
};
export const organizationAgent = {
  access: 'shared',
  id: 'agent-1',
  status: 'ready',
  title: 'Organization Agent',
  created_at: organizationTime,
  updated_at: organizationTime,
};
export const organizationUsage = {
  agent_id: 'agent-1',
  channel_usage: {
    active: true,
    has_history: true,
    first_used_at: organizationTime,
    last_used_at: organizationTime,
  },
};

export function organizationResponse(url: URL) {
  if (url.pathname.includes('/resource-access/')) return organizationAccess;
  if (url.pathname.endsWith('/sandboxes')) return { items: [organizationSandbox] };
  if (url.pathname.endsWith('/sandboxes/sandbox-1')) return organizationSandbox;
  if (url.pathname.endsWith('/sandboxes/sandbox-1/workspaces'))
    return { items: [organizationWorkspaceSummary] };
  if (url.pathname.endsWith('/workspaces/workspace-1')) return organizationWorkspace;
  if (url.pathname.endsWith('/agents/agent-1/channel-usage')) return organizationUsage;
  if (url.pathname.endsWith('/agents/agent-1/provenance'))
    return {
      agent_id: 'agent-1',
      created_at: organizationTime,
      creation_origin: 'manual',
    };
  if (url.pathname.endsWith('/agents/agent-1')) return organizationAgent;
  return undefined;
}
