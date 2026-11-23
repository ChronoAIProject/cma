import { render } from '@testing-library/react';
import { expect, vi } from 'vitest';
import { createClient } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { Console } from '../Console';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { WIRE_AGENT } from '../test/rest';

export const at = '2026-09-01T00:00:00Z';
export const sandbox = {
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
export const workspace = {
  access: 'shared',
  id: 'wks_shared',
  sandbox_id: sandbox.id,
  name: 'Review Workspace',
  status: 'ready',
  created_at: at,
  worktree_count: 1,
  agent_count: 1,
};
export const worktree = {
  access: 'shared',
  id: 'wt_shared',
  sandbox_id: sandbox.id,
  workspace_id: workspace.id,
  name: 'Review branch',
  status: 'ready',
  agent_id: 'agt_shared',
};
export const agent = {
  id: 'agt_shared',
  worktree_id: worktree.id,
  sandbox_id: sandbox.id,
  workspace_id: workspace.id,
  status: 'ready',
  title: 'Review conversation',
  updated_at: at,
};

export function fixture(shared = false) {
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
    if (url.pathname.endsWith('/agents/provenance')) {
      expect(url.searchParams.get('shared')).toBe('true');
      return Response.json({ items: [] });
    }
    if (url.pathname.endsWith('/agents/agt_1'))
      return Response.json({
        ...WIRE_AGENT,
        id: AGENT_DETAIL.id,
        worktree: { ...WIRE_AGENT.worktree, id: AGENT_DETAIL.worktree.id },
        workspace: { ...WIRE_AGENT.workspace, id: AGENT_DETAIL.workspace.id },
        sandbox: { ...WIRE_AGENT.sandbox, id: AGENT_DETAIL.sandbox.id },
      });
    if (state.revoked)
      return Response.json(
        { code: 'resource_not_found', detail: 'Access removed' },
        { status: 404 },
      );
    if (url.pathname === '/api/v1/agents/agt_shared/channel-usage')
      return Response.json({
        agent_id: 'agt_shared',
        channel_usage: { active: true, has_history: true, first_used_at: null, last_used_at: null },
      });
    if (url.pathname.includes('/resource-access/'))
      return Response.json({
        capabilities: ['view_details'],
        revalidate_at: null,
        policy_revision: state.policy,
      });
    if (url.pathname === '/api/v1/sandboxes') {
      expect(url.searchParams.get('organization')).toBe(shared ? null : 'org_research');
      expect(url.searchParams.get('shared')).toBe(shared ? 'true' : null);
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
