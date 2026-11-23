import type { Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

export const redesignTime = '2026-11-22T06:00:00Z';
export type HomeState =
  | 'crew'
  | 'empty'
  | 'near'
  | 'full'
  | 'capacity-error'
  | 'access-error'
  | 'accepted'
  | 'stuck'
  | 'navigation-error';

export async function redesignHomeFixture(
  page: Page,
  baseURL: string,
  state: HomeState = 'crew',
  templateSupport = true,
) {
  const boundary = new LocalStartup();
  const writes: { body: Record<string, unknown>; key?: string }[] = [];
  const reads: string[] = [];
  let capacityFailed = state === 'capacity-error';
  let organizationsFailed = state === 'access-error';
  let statusFailed = state === 'stuck';
  await page.clock.install({ time: new Date(redesignTime) });
  await boundary.route(page, baseURL);
  const parents = [
    boundary.sandbox,
    { ...boundary.sandbox, id: 'sbx_second', name: 'Research sandbox' },
    { ...boundary.sandbox, id: 'sbx_third', name: 'Planning sandbox', status: 'paused' as const },
  ];
  const workspaces = parents.map((parent, i) => {
    const id = i === 0 ? boundary.workspace.id : `wks_redesign_${i}`;
    const agent = {
      ...boundary.workspace.agents[0]!,
      id: i === 0 ? boundary.agent.id : `agt_redesign_${i}`,
      sandbox_id: parent.id,
      workspace_id: id,
      status: i === 0 ? ('working' as const) : ('ready' as const),
      task_status: i === 0 ? ('working' as const) : ('not_started' as const),
      title: ['Sample task', 'Example research', 'Project outline'][i]!,
      updated_at: ['2026-11-22T05:59:00Z', '2026-11-22T05:48:00Z', '2026-11-21T06:00:00Z'][i]!,
      task_activity_at: ['2026-11-22T05:59:00Z', '2026-11-22T05:48:00Z', '2026-11-21T06:00:00Z'][
        i
      ]!,
    };
    return { ...boundary.workspace, id, sandbox_id: parent.id, agents: [agent] };
  });
  await page.route('**/openapi.json', (route) =>
    route.fulfill({
      json: {
        openapi: '3.1.0',
        info: { title: 'Synthetic', version: '1' },
        paths: {},
        'x-cma-task-templates-v1': templateSupport,
        'x-cma-task-templates-v2': templateSupport,
        'x-cma-agent-title-mode': true,
        'x-cma-task-repository': true,
        components: {
          schemas: {
            AgentLifecycleCreateRequest: {
              properties: { initial_message: { type: 'string' }, title_mode: { type: 'string' } },
            },
          },
        },
      },
    }),
  );
  await page.route('**/api/v2/ui/task-experience', (route) =>
    route.fulfill({
      json: {
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: 'local-owner',
        capabilities: {
          task_home_admission: true,
          desktop_enable: false,
          suggested_followup_admission: false,
          browser_takeover: false,
          replay_presentation: false,
        },
      },
    }),
  );
  await page.route('**/api/v1/**', (route) => {
    const path = new URL(route.request().url()).pathname;
    reads.push(path);
    const reply = (json: unknown, status = 200) => route.fulfill({ status, json });
    if (path === '/api/v1/sandboxes/capacity') {
      if (capacityFailed) return reply({ code: 'unavailable' }, 503);
      const live = state === 'full' ? 5 : state === 'near' ? 4 : 3;
      return reply({
        resource: 'sandbox',
        limit: 5,
        live,
        available: 5 - live,
        counts_by_status: state === 'full' ? { active: 3, retiring: 2 } : { active: live },
        observed_at: redesignTime,
      });
    }
    if (path === '/api/v1/organizations')
      return organizationsFailed ? reply({ code: 'unavailable' }, 503) : reply({ items: [] });
    if (path === '/api/v1/sandboxes')
      return state === 'navigation-error'
        ? reply({ code: 'unavailable' }, 503)
        : reply({ items: state === 'empty' ? [] : parents });
    const parent = parents.find((item) => path === `/api/v1/sandboxes/${item.id}`);
    if (parent)
      return reply({
        ...parent,
        workspaces: [workspaces.find((item) => item.sandbox_id === parent.id)!],
      });
    const workspace = workspaces.find((item) => path === `/api/v1/workspaces/${item.id}`);
    if (workspace) return reply(workspace);
    if (path === '/api/v1/agents/provenance') return reply({ items: [] });
    if (
      workspaces.some((workspace) =>
        workspace.agents.some((agent) => path === `/api/v1/agents/${agent.id}/reply-preview`),
      )
    )
      return reply({ code: 'not_found' }, 404);
    const agent = workspaces
      .flatMap((item) => item.agents)
      .find(
        (item) =>
          path === `/api/v1/agents/${item.id}/channel-usage` ||
          path === `/api/v1/agents/${item.id}/provenance`,
      );
    if (agent)
      return path.endsWith('/channel-usage')
        ? reply({
            agent_id: agent.id,
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          })
        : reply({
            agent_id: agent.id,
            created_at: redesignTime,
            creation_origin: 'legacy_unknown',
          });
    return route.fallback();
  });
  await page.route('**/api/agent-manager/v1/agents', (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    writes.push({
      body: route.request().postDataJSON(),
      key: route.request().headers()['idempotency-key'],
    });
    return route.fulfill({ status: 202, json: { id: 'redesign-operation', status: 'queued' } });
  });
  await page.route('**/api/agent-manager/v1/agent-lifecycle/redesign-operation', (route) =>
    statusFailed
      ? route.fulfill({ status: 503, json: { code: 'unavailable' } })
      : route.fulfill({
          json: {
            id: 'redesign-operation',
            status: 'running',
            phase: 'sandbox_runtime',
            events: [
              {
                sequence: 1,
                phase: 'sandbox_ready',
                status: 'running',
                message: 'Preparing the environment',
                occurred_at: redesignTime,
              },
            ],
          },
        }),
  );
  return {
    boundary,
    writes,
    reads,
    capacityRecovers: () => {
      capacityFailed = false;
    },
    organizationsRecover: () => {
      organizationsFailed = false;
    },
    failStatus: () => {
      statusFailed = true;
    },
  };
}
