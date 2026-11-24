import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import type { Page, TestInfo } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { LocalAgentV2 } from './agent-v2.fixture';
import { workingAgent } from './redesign-agent.fixture';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { flowProbe } from './topographic-flow.probes';

export async function flowFixture(
  page: Page,
  baseURL: string,
  target: 'landing' | 'first-visit' | 'ordinary',
) {
  const boundary = new LocalStartup();
  const signIns: string[] = [];
  await boundary.route(page, baseURL);
  if (target === 'landing') {
    await page.route('**/auth/session', (route) =>
      route.fulfill({ json: { authenticated: false, can_silent_sso: false } }),
    );
    await page.route('**/auth/login**', (route) => {
      signIns.push(new URL(route.request().url()).searchParams.get('return_to') ?? '');
      return route.fulfill({ contentType: 'text/html', body: '<p>Local sign-in destination</p>' });
    });
  }
  if (target === 'first-visit')
    await page.route(
      (url) => url.pathname === '/api/v1/sandboxes',
      (route) => route.fulfill({ json: { items: [] } }),
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
  const chunks: string[] = [];
  page.on('request', (request) => {
    if (/topographic-surface[.-]/.test(request.url())) chunks.push(new URL(request.url()).pathname);
  });
  return { boundary, signIns, chunks };
}

export async function protectedFlowFixture(
  page: Page,
  baseURL: string,
  target: 'ordinary' | 'running' | 'deletion' | 'usage' | 'channel',
) {
  const fixture = await flowFixture(page, baseURL, 'ordinary');
  const task = ['running', 'deletion', 'channel'].includes(target) ? new LocalAgentV2() : null;
  if (task) {
    if (target === 'running') workingAgent(task);
    await task.route(page, baseURL);
  }
  const boundary = task?.boundary ?? fixture.boundary;
  if (target === 'usage') {
    const metrics = new LocalUsageMetrics();
    await metrics.route(page);
    await metrics.session(page);
  }
  if (target === 'deletion') {
    boundary.sandbox = { ...boundary.sandbox, status: 'retiring' };
    await page.addInitScript((id) => {
      localStorage.setItem(
        'cma:deletion:local-owner',
        JSON.stringify({
          key: 'synthetic-flow-deletion',
          accepted: true,
          action: { kind: 'delete_sandbox', sandboxId: id },
        }),
      );
    }, boundary.sandbox.id);
  }
  if (target === 'channel') {
    await page.route(
      (url) => url.pathname === '/api/v1/sandboxes',
      (route) => route.fulfill({ json: { items: [] } }),
    );
    await page.route('**/api/v1/agents/agt_contract', (route) =>
      route.fulfill({
        json: {
          access: 'shared',
          id: 'agt_contract',
          title: 'Shared channel task',
          status: 'ready',
          created_at: localTime,
          updated_at: localTime,
        },
      }),
    );
    await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
      route.fulfill({
        json: {
          capabilities: ['view_details', 'view_history', 'manage_bridge'],
          revalidate_at: null,
          policy_revision: 1,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/channel-usage**', (route) =>
      route.fulfill({
        json: new URL(route.request().url()).pathname.endsWith('/history')
          ? { agent_id: 'agt_contract', items: [] }
          : {
              agent_id: 'agt_contract',
              channel_usage: {
                active: false,
                has_history: true,
                first_used_at: null,
                last_used_at: null,
              },
              bindings: [],
            },
      }),
    );
  }
  const surface =
    target === 'ordinary'
      ? page.locator('.task-home')
      : target === 'running'
        ? page.getByRole('heading', { name: 'Working on it', exact: true })
        : target === 'usage'
          ? page.getByRole('region', { name: 'Allocation and history overview', exact: true })
          : page.getByRole('dialog', {
              name: target === 'deletion' ? 'Deleting Sandbox' : 'Channel history',
              exact: true,
            });
  return {
    ...fixture,
    boundary,
    surface,
    taskWrites: task?.writes ?? [],
    async open() {
      await page.goto(
        target === 'ordinary' ? '/' : target === 'usage' ? '/usage' : '/agents/agt_contract',
      );
      if (target === 'deletion') {
        await page
          .getByRole('region', { name: 'Deletion request', exact: true })
          .getByRole('button', { name: 'View status', exact: true })
          .click();
        await page
          .getByRole('button', { name: 'View Sandbox deletion status (1)', exact: true })
          .click();
      }
      if (target === 'channel') {
        await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
        await page.getByRole('button', { name: 'Channel history', exact: true }).click();
      }
    },
    async close() {
      if (task) await task.close(page);
    },
  };
}

export async function captureFlow(page: Page, info: TestInfo, label: string) {
  const path = info.outputPath(`${label}.png`);
  await page.screenshot({ path });
  const metadata = await page.evaluate(() => ({
    route: location.pathname,
    width: innerWidth,
    height: innerHeight,
    dpr: devicePixelRatio,
    dark: document.body.hasAttribute('data-ds-dark-theme'),
    reduced: matchMedia('(prefers-reduced-motion: reduce)').matches,
  }));
  await writeFile(
    info.outputPath(`${label}.json`),
    JSON.stringify(
      {
        ...metadata,
        flag: 'true',
        source: process.env.CMA_SOURCE_SHA ?? 'working-tree',
        fixture: 'synthetic-local-owner',
        screenshotSha256: createHash('sha256')
          .update(await readFile(path))
          .digest('hex'),
        probe: await flowProbe(page),
      },
      null,
      2,
    ),
  );
}
