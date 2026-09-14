import { readFileSync } from 'node:fs';
import type { Page } from '@playwright/test';
import type {
  Agent,
  SandboxDetail,
  Workspace,
  WorkspaceDetail,
} from '../../sdk/typescript/src/index';

export function recorded<T>(file: string, id: string): T {
  const corpus = JSON.parse(
    readFileSync(
      new URL(
        `../../api/crates/layer1-public/tests/fixtures/public-contract/${file}`,
        import.meta.url,
      ),
      'utf8',
    ),
  ) as { cases: { id: string; response: { body: T } }[] };
  const entry = corpus.cases.find((item) => item.id === id);
  if (!entry) throw new Error(`Missing local public fixture ${id}`);
  return entry.response.body;
}

export const localTime = '2026-09-05T00:00:00Z';
export interface LocalEvent {
  seq: number;
  appended_at: string;
  kind: Record<string, unknown>;
}

// Models only the public boundary; no provider or backend execution is claimed.
export class LocalStartup {
  sandbox = recorded<SandboxDetail>('instances.json', 'sandbox-read');
  workspace = recorded<WorkspaceDetail>('instances.json', 'workspace-read');
  agent = recorded<Agent>('agents.json', 'agent-read');
  events: LocalEvent[] = [];
  writes: { path: string; key?: string; body: unknown }[] = [];
  unexpected: string[] = [];
  errors: string[] = [];
  loseCreationResponse = false;
  private creation?: unknown;
  private createdWorkspace?: WorkspaceDetail;

  async route(page: Page, baseURL: string) {
    page.on('pageerror', (error) => this.errors.push(error.message));
    await page.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const reply = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
      if (url.origin !== baseURL) {
        this.unexpected.push('external_request');
        return route.abort();
      }
      if (url.pathname === '/auth/session')
        return reply({
          authenticated: true,
          subject: 'local-owner',
          display_name: 'Local owner',
          capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
        });
      if (!url.pathname.startsWith('/api/')) return route.continue();
      if (request.method() === 'POST') {
        this.writes.push({
          path: url.pathname,
          key: request.headers()['idempotency-key'],
          body: request.postDataJSON(),
        });
        if (url.pathname === '/api/v1/workspaces/wks_route/agents') {
          this.agent = {
            ...this.agent,
            status: 'starting',
            sandbox: { ...this.agent.sandbox, status: 'reviving' },
            first_message: { state: 'queued', queued_at: localTime },
          };
          this.creation ??= structuredClone(this.agent);
        } else if (url.pathname === '/api/v1/sandboxes/sbx_route/workspaces') {
          if (!this.createdWorkspace) {
            const created = recorded<Workspace>(
              'instances-state.json',
              'state-workspace-parent-paused',
            );
            this.creation = created;
            this.createdWorkspace = {
              ...created,
              worktrees: [],
              agents: [],
              worktree_count: 0,
              agent_count: 0,
            };
            this.sandbox = {
              ...this.sandbox,
              workspaces: [
                ...this.sandbox.workspaces,
                { ...created, worktree_count: 0, agent_count: 0 },
              ],
            };
          }
        } else {
          this.unexpected.push(`POST ${url.pathname}`);
          return reply({ code: 'unexpected_local_write' }, 500);
        }
        this.sandbox = { ...this.sandbox, status: 'reviving' };
        if (this.loseCreationResponse) {
          this.loseCreationResponse = false;
          return route.abort('connectionfailed');
        }
        return reply(this.creation, 201);
      }
      if (request.method() !== 'GET') {
        this.unexpected.push(`${request.method()} ${url.pathname}`);
        return reply({ code: 'unexpected_local_write' }, 500);
      }
      if (
        this.createdWorkspace &&
        url.pathname === `/api/v1/workspaces/${this.createdWorkspace.id}`
      )
        return reply(this.createdWorkspace);
      const profile =
        /^\/api\/v1\/(sandbox|workspace|agent)-profiles\/([^/]+)(?:\/revisions\/(\d+))?$/.exec(
          url.pathname,
        );
      if (profile) {
        const [, kind, id, revision] = profile;
        return reply(
          revision === undefined
            ? {
                ...recorded<object>('profiles.json', `${kind}-profile-read`),
                id,
                name: `${kind} profile`,
              }
            : {
                ...recorded<object>('profiles.json', `${kind}-profile-exact-revision`),
                profile_id: id,
                revision: Number(revision),
              },
        );
      }
      switch (url.pathname) {
        case '/api/v1/profile-authoring/services':
          return reply([{ id: 'api-github', catalog_id: 'catalog-github', name: 'GitHub' }]);
        case '/api/v1/profile-authoring/skills':
          return reply({
            items: [{ id: url.searchParams.get('q'), name: 'TypeScript', versions: ['1.2'] }],
          });
        case '/api/v1/agent-profiles':
        case '/api/v1/workspace-profiles':
          return reply({ items: [] });
        case '/api/v1/defaults':
          return reply({
            sandbox_profile: { id: 'sbp_route', revision: 1 },
            workspace_profile: { id: 'wkp_route', revision: 1 },
            agent_profile: { id: 'agp_route', revision: 1 },
          });
        case '/api/v1/agent-profiles/agp_route/revisions/1':
          return reply({
            ...recorded<object>('profiles.json', 'agent-profile-exact-revision'),
            profile_id: 'agp_route',
          });
        case '/api/v1/workspace-profiles/wkp_route/revisions/1':
          return reply({
            ...recorded<object>('profiles.json', 'workspace-profile-exact-revision'),
            profile_id: 'wkp_route',
          });
        case '/api/v1/sandboxes':
          return reply({ items: [this.sandbox] });
        case '/api/v1/sandboxes/sbx_route':
          return reply(this.sandbox);
        case '/api/v1/sandboxes/sbx_route/workspaces':
          return reply({ items: this.sandbox.workspaces });
        case '/api/v1/sandboxes/sbx_route/events':
          return reply([]);
        case '/api/v1/workspaces/wks_route':
          return reply(this.workspace);
        case '/api/v1/workspaces/wks_route/worktrees':
          return reply({ items: this.workspace.worktrees });
        case '/api/v1/agents':
          return reply({ items: this.workspace.agents });
        case '/api/v1/agents/agt_contract':
          return reply(this.agent);
        case '/api/v1/agents/agt_contract/transcript':
          return reply({
            agent_id: this.agent.id,
            through_seq: this.events.at(-1)?.seq ?? 0,
            turns: [],
          });
        case '/api/v1/agents/agt_contract/usage':
          return reply({
            agent_id: this.agent.id,
            through_seq: this.events.at(-1)?.seq ?? 0,
            input_tokens: 0,
            output_tokens: 0,
            context_used: 0,
          });
        case '/api/v1/agents/agt_contract/events':
          return reply({
            items: this.events.filter(
              (event) => event.seq > Number(url.searchParams.get('after') ?? 0),
            ),
          });
        case '/api/v1/agui/events':
          return route.fulfill({
            contentType: 'text/event-stream',
            body: ': local replay has no new frames\n\n',
          });
        default:
          this.unexpected.push(`GET ${url.pathname}`);
          return reply({ code: 'unexpected_local_read' }, 404);
      }
    });
  }
}
