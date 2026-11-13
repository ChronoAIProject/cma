import { expect, type Page, type Route } from '@playwright/test';
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

export async function retainedAssets(page: Page, directory: string, agentId: string) {
  const root = process.env.CMA_INTEGRATED_RETAINED_DIST;
  if (!root) return;
  const before = await (await page.request.get(`/api/v1/agents/${agentId}/transcript`)).json();
  const answer = page.getByRole('article').filter({ hasText: 'The comparison is complete.' });
  await page.getByRole('tab', { name: 'Terminal', exact: true }).click();
  const transportKey = `cma:agent-transport:v1:owner:${agentId}`;
  const stored = () => page.evaluate((key) => sessionStorage.getItem(key), transportKey);
  const original = await stored();
  expect(JSON.parse(original!).transport).toBe('responses_v2');
  const served: { path: string; sha256: string }[] = [];
  const useOld = async (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (!request.isNavigationRequest() && !path.startsWith('/assets/')) return route.continue();
    const file = request.isNavigationRequest() ? 'index.html' : path.slice(1);
    const absolute = resolve(root, file);
    expect(absolute.startsWith(resolve(root) + '/')).toBe(true);
    const body = await readFile(absolute);
    served.push({ path: file, sha256: createHash('sha256').update(body).digest('hex') });
    const contentType = file.endsWith('.html')
      ? 'text/html'
      : file.endsWith('.js')
        ? 'text/javascript'
        : file.endsWith('.css')
          ? 'text/css'
          : file.endsWith('.svg')
            ? 'image/svg+xml'
            : file.endsWith('.png')
              ? 'image/png'
              : 'application/octet-stream';
    await route.fulfill({ status: 200, contentType, body });
  };
  await page.route('**/*', useOld);
  await page.reload();
  await expect(
    page.getByText('Maintenance effort compared for all three tools.', { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Agent computer' })).toBeVisible();
  await page.getByRole('tab', { name: /^(Operation output|Terminal)$/ }).click();
  expect(await stored()).toEqual(original);
  await expect(
    answer.getByRole('button', { name: 'Download report.txt', exact: true }),
  ).toBeEnabled();
  await expect(
    answer.getByRole('button', { name: 'Needs improvement', exact: true }),
  ).toHaveAttribute('aria-pressed', 'true');
  await page.screenshot({ path: join(directory, 'retained-assets.png'), fullPage: true });
  await page.unroute('**/*', useOld);
  await page.reload();
  await expect(
    page.getByText('Maintenance effort compared for all three tools.', { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'Agent computer' })).toBeVisible();
  await page.getByRole('tab', { name: /^(Operation output|Terminal)$/ }).click();
  expect(await stored()).toEqual(original);
  await expect(page.locator('#computer-page-activity').getByRole('article').first()).toBeVisible();
  const after = await (await page.request.get(`/api/v1/agents/${agentId}/transcript`)).json();
  expect(after).toEqual(before);
  await writeFile(
    join(directory, 'retained-assets-result.json'),
    JSON.stringify(
      {
        passed: true,
        root,
        served,
        original,
        after: await stored(),
        transcriptUnchanged: true,
        boundary:
          'Actual retained17 static bytes and current assets on one origin/tab/storage; current public server and pinned native process continue unchanged.',
      },
      null,
      2,
    ),
  );
}
