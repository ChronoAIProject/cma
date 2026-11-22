import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import type { Page, TestInfo } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
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
    await page.route('**/api/v1/sandboxes', (route) => route.fulfill({ json: { items: [] } }));
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
