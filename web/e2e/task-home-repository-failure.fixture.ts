import type { Page } from '@playwright/test';
import type { AgentLifecycleCreateRequest } from '../src/api-client/index';
import type { ConnectionRow } from '../src/connections/client';
import { LocalStartup, localTime } from './agent-startup-fixtures';

export const taskText = 'Read the synthetic repository and summarize its tests.';
export const repository = { url: 'https://github.com/example/project', ref: 'main' };

// This public-boundary model checks browser writes, not real runtime dispatch or GitHub access.
export class RepositoryTaskFixture extends LocalStartup {
  outcome: 'failed' | 'succeeded' = 'failed';
  failure = 'repository_connection_required: Repository preparation was refused.';
  connection: ConnectionRow['state'] = 'satisfied';
  preflight = false;
  operationId = 'repository-operation';
  submissions: { key: string; body: AgentLifecycleCreateRequest }[] = [];
  checks: unknown[] = [];
  allocations = new Set<string>();
  dispatched = new Set<string>();
  private readonly knownAgents = new Set([this.agent.id]);
  private waiting = false;

  override async route(page: Page, baseURL: string) {
    await super.route(page, baseURL);
    await page.route('**/api/v1/agents/*/channel-usage', (route) => {
      const id = new URL(route.request().url()).pathname.split('/')[4]!;
      if (!this.knownAgents.has(id)) return route.fallback();
      return route.fulfill({
        json: {
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
    await page.route('**/api/v2/ui/task-experience', (route) =>
      route.fulfill({
        json: {
          object: 'cma.ui.task_experience',
          schema_version: 1,
          account_id: 'local-owner',
          capabilities: {
            task_home_admission: true,
            desktop_enable: true,
            suggested_followup_admission: false,
            browser_takeover: false,
            replay_presentation: false,
          },
        },
      }),
    );
    await page.route('**/auth/connections/check', (route) => {
      this.checks.push(route.request().postDataJSON());
      return route.fulfill({
        json: {
          owner: 'local-owner',
          ready: this.connection === 'satisfied',
          checked_at: localTime,
          services: [
            {
              service: 'api-github',
              display_name: 'GitHub',
              kind: 'nyxid',
              state: this.connection,
              connection_mode: this.connection === 'unknown' ? 'unknown' : 'hosted',
            },
          ],
        },
      });
    });
    await page.route('**/api/agent-manager/v1/agents', (route) => {
      const request = route.request();
      const body = request.postDataJSON() as AgentLifecycleCreateRequest;
      const key = request.headers()['idempotency-key'];
      if (request.method() !== 'POST' || !key) throw new Error('Expected an explicit keyed task');
      this.submissions.push({ key, body });
      this.waiting = this.preflight && this.connection !== 'satisfied';
      if (!this.waiting) this.allocations.add(key);
      if (!this.waiting && this.outcome === 'succeeded') {
        this.dispatched.add(key);
        this.agent = {
          ...this.agent,
          id: `agt_repository_${this.dispatched.size}`,
          worktree: { ...this.agent.worktree, id: `wkt_repository_${this.dispatched.size}` },
          first_message: {
            state: 'completed',
            queued_at: localTime,
            completed_at: localTime,
          },
        };
        this.knownAgents.add(this.agent.id);
      }
      return route.fulfill({ status: 202, json: { id: this.operationId, status: 'provisioning' } });
    });
    await page.route('**/api/agent-manager/v1/agent-lifecycle/*', (route) =>
      route.fulfill({
        json: {
          id: this.operationId,
          status: this.waiting ? 'waiting_for_connection' : this.outcome,
          phase: this.waiting ? 'connections' : this.outcome,
          ...(!this.waiting ? { agent_id: this.agent.id } : {}),
          ...(this.waiting
            ? {
                connection: {
                  services: ['api-github'],
                  target: { type: 'sandbox_profile', profile: { id: 'sbp_route', revision: 3 } },
                },
              }
            : {}),
          ...(!this.waiting && this.outcome === 'failed' ? { error: this.failure } : {}),
          events: this.waiting
            ? []
            : [
                {
                  sequence: 1,
                  phase: 'workspace_runtime',
                  status: this.outcome,
                  message: this.outcome === 'failed' ? this.failure : 'Repository ready.',
                  occurred_at: localTime,
                },
              ],
        },
      }),
    );
  }
}

export async function submitRepository(page: Page) {
  await page.goto('/');
  await page.getByRole('button', { name: 'Add git repo' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add git repo' });
  await dialog.getByLabel('GitHub repository').fill(repository.url);
  await dialog.getByLabel('Repository ref').fill(repository.ref);
  await dialog.getByRole('button', { name: 'Add repo', exact: true }).click();
  await page.getByLabel('Your task').fill(taskText);
  await page.getByRole('button', { name: 'Start task' }).click();
}

export function retainedTask(page: Page) {
  return page.evaluate(() => {
    const key = `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`;
    return sessionStorage.getItem(key);
  });
}
