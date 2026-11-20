import type { Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

export async function newAgentRequestFixture(
  page: Page,
  baseURL: string,
  status: 'active' | 'provisioning',
) {
  const fixture = new LocalStartup();
  fixture.agent = {
    ...fixture.agent,
    status: 'starting',
    sandbox: { ...fixture.agent.sandbox, status },
    bootstrap: {
      operation_id: 'old-operation',
      sandbox_id: fixture.sandbox.id,
      workspace_id: fixture.workspace.id,
    },
  };
  fixture.sandbox = { ...fixture.sandbox, status };
  await fixture.route(page, baseURL);
  await page.route(`${baseURL}/api/v2/ui/task-experience`, (route) =>
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
  let delay = false;
  let held = false;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  await page.route(
    `${baseURL}/api/agent-manager/v1/agent-lifecycle/old-operation`,
    async (route) => {
      if (delay) {
        held = true;
        await gate;
      }
      await route.fulfill({
        json: {
          id: 'old-operation',
          status: 'running',
          phase: 'sandbox_runtime',
          agent_id: fixture.agent.id,
          events: [
            {
              sequence: 1,
              phase: 'sandbox_ready',
              status: 'running',
              message: 'Preparing synthetic Sandbox',
              occurred_at: new Date().toISOString(),
            },
          ],
        },
      });
    },
  );
  const creates: { body: unknown; key?: string }[] = [];
  await page.route(`${baseURL}/api/agent-manager/v1/agents`, async (route) => {
    if (route.request().method() !== 'POST') return route.fallback();
    creates.push({
      body: route.request().postDataJSON(),
      key: route.request().headers()['idempotency-key'],
    });
    await route.fulfill({ status: 202, json: { id: 'new-operation', status: 'queued' } });
  });
  await page.route(`${baseURL}/api/agent-manager/v1/agent-lifecycle/new-operation`, (route) =>
    route.fulfill({
      json: { id: 'new-operation', status: 'queued', phase: 'sandbox_runtime', events: [] },
    }),
  );
  await page.addInitScript(
    ({ agentId }) => {
      if (sessionStorage.getItem('fixture-seeded')) return;
      sessionStorage.setItem('fixture-seeded', 'true');
      sessionStorage.setItem(
        `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`,
        JSON.stringify({
          version: 1,
          intent: {
            key: 'retained-old-request',
            body: { name: 'Old Agent' },
            receipt: { id: 'old-operation', status: 'running' },
            agentId,
          },
        }),
      );
    },
    { agentId: fixture.agent.id },
  );
  return {
    fixture,
    creates,
    delay: () => {
      delay = true;
    },
    held: () => held,
    release: () => {
      delay = false;
      release();
    },
  };
}
