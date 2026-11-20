import type { Page } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import type { SandboxDetail } from '../src/api-client';

export class LocalSandboxFilters {
  readonly boundary = new LocalStartup();
  viewer = 'local-owner';
  paginated = true;
  readonly writes: string[] = [];
  readonly lists: URL[] = [];
  readonly sandboxes: SandboxDetail[] = (
    [
      'provisioning',
      'installing',
      'retiring',
      'retired',
      'failed',
      'paused',
      'pausing',
      'reviving',
      'credential_rotating',
      'rotating',
    ] as const
  ).map((status, i) => ({
    ...this.boundary.sandbox,
    id: `sbx_filter_${i}`,
    name: `${status} fixture`,
    status,
    workspaces: [],
    workspace_count: 0,
    worktree_count: 0,
    agent_count: 0,
  }));
  async route(page: Page, baseURL: string) {
    await this.boundary.route(page, baseURL);
    await page.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== baseURL) return route.fallback();
      const reply = (json: unknown) => route.fulfill({ json });
      if (request.method() !== 'GET' && /^\/(api|auth)\//.test(url.pathname)) {
        this.writes.push(`${request.method()} ${url.pathname}`);
        return route.fulfill({ status: 500, json: { code: 'unexpected_write' } });
      }
      if (url.pathname === '/auth/session')
        return reply({
          authenticated: true,
          subject: this.viewer,
          display_name: 'Synthetic owner',
          capabilities: ['use_own_sandboxes'],
        });
      if (url.pathname === '/auth/connections/check')
        return reply({ owner: this.viewer, ready: true, checked_at: localTime, services: [] });
      if (url.pathname === '/api/v1/organizations')
        return reply({
          items: [{ id: 'research', display_name: 'Synthetic research', role: 'member' }],
        });
      if (url.pathname === '/api/v1/sandboxes') {
        this.lists.push(url);
        if (!this.paginated)
          return reply({
            items: [this.boundary.sandbox, ...this.sandboxes].sort((a, b) =>
              a.id.localeCompare(b.id),
            ),
          });
        if (url.searchParams.has('shared') || url.searchParams.has('organization'))
          return reply(
            url.searchParams.has('cursor')
              ? { items: [this.sandboxes[1]] }
              : { items: [this.sandboxes[0]], next: 'sbx_filter_0' },
          );
        return reply({ items: [this.boundary.sandbox, ...this.sandboxes] });
      }
      const sandbox = this.sandboxes.find((row) => url.pathname === `/api/v1/sandboxes/${row.id}`);
      if (sandbox) return reply(sandbox);
      if (
        url.pathname.startsWith('/api/v1/resource-access/sandbox/sbx_filter_') ||
        url.pathname === '/api/v1/resource-access/workspace/wks_route'
      )
        return reply({
          capabilities: ['view_details'],
          owner: { kind: 'person', is_current_person: false },
          revalidate_at: null,
          policy_revision: 1,
        });
      if (url.pathname === '/api/v1/agents/provenance') return reply({ items: [] });
      return route.fallback();
    });
  }
}
