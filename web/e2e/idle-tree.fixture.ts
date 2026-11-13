import type { Page } from '@playwright/test';
import type { LocalAgentV2 } from './agent-v2.fixture';

/** Twelve Agents in six Workspaces and three Sandboxes, using public shapes. */
export async function installIdleTree(page: Page, fixture: LocalAgentV2) {
  const workspaces = Array.from({ length: 6 }, (_, index) => {
    const sandboxId = index < 2 ? 'sbx_route' : `sbx_fixture_${Math.floor(index / 2)}`;
    const id = index === 0 ? 'wks_route' : `wks_fixture_${index}`;
    const agents = Array.from({ length: 2 }, (_, child) => {
      const agentId = index === 0 && child === 0 ? 'agt_contract' : `agt_fixture_${index}_${child}`;
      return {
        ...fixture.boundary.workspace.agents[0]!,
        id: agentId,
        sandbox_id: sandboxId,
        workspace_id: id,
        worktree_id: `wkt_${agentId}`,
      };
    });
    return {
      ...fixture.boundary.workspace,
      id,
      sandbox_id: sandboxId,
      agent_count: 2,
      worktree_count: 2,
      agents,
      worktrees: agents.map((agent) => ({
        ...fixture.boundary.workspace.worktrees[0]!,
        id: agent.worktree_id,
        agent_id: agent.id,
        workspace_id: id,
        sandbox_id: sandboxId,
      })),
    };
  });
  const sandboxes = Array.from({ length: 3 }, (_, index) => ({
    ...fixture.boundary.sandbox,
    id: index === 0 ? 'sbx_route' : `sbx_fixture_${index}`,
    workspace_count: 2,
    worktree_count: 4,
    agent_count: 4,
    workspaces: workspaces.slice(index * 2, index * 2 + 2),
  }));
  await page.route(/\/api\/v1\/(sandboxes|workspaces)(\/[^/?]+)?(\?.*)?$/, async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/api/v1/sandboxes') return route.fulfill({ json: { items: sandboxes } });
    const item = [...sandboxes, ...workspaces].find((value) => path.endsWith(`/${value.id}`));
    if (item) return route.fulfill({ json: item });
    return route.fallback();
  });
  await page.route(/\/api\/v1\/agents\/[^/]+\/(provenance|channel-usage)$/, (route) => {
    const path = new URL(route.request().url()).pathname;
    const id = path.split('/')[4]!;
    return route.fulfill({
      json: path.endsWith('/provenance')
        ? {
            agent_id: id,
            created_at: fixture.started,
            creation_origin: 'manual',
          }
        : {
            agent_id: id,
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          },
    });
  });
}
