import { test, expect } from '@playwright/test';
import type { Launch } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

test('LOCAL Check again retains newer Launch progress when a read returns an older revision', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const historical = recorded<Launch>('launches.json', 'launch-ready-start');
  const failed: Launch = {
    ...historical,
    state: 'failed_terminal',
    revision: 3,
    event_head: 5,
    failure: {
      code: 'launch_failed',
      safe_detail: 'Synthetic progress failure',
      retryable: false,
      occurred_at: historical.updated_at,
    },
  };
  const cancelled: Launch = {
    ...failed,
    state: 'cancelled',
    revision: 4,
    event_head: 7,
    failure: undefined,
  };
  const reads: { revision: number; state: string }[] = [];
  let streams = 0;
  let recovered = false;
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin === baseURL && url.pathname === `/api/v1/agui/launches/${failed.id}/events`) {
      streams += 1;
      return route.abort('failed');
    }
    if (
      url.origin !== baseURL ||
      url.pathname !== `/api/v1/launches/${failed.id}` ||
      route.request().method() !== 'GET'
    )
      return route.fallback();
    const value = reads.length === 0 ? failed : recovered ? cancelled : historical;
    reads.push({ revision: value.revision, state: value.state });
    return route.fulfill({ contentType: 'application/json', body: JSON.stringify(value) });
  });
  await page.goto(`/launches/${failed.id}`);
  const badge = page.locator('.public-progress > .badge');
  await expect(badge).toHaveText("Didn't work");
  const observation = page.locator('.public-observed');
  const lastChecked = (await observation.innerText()).replace('Last checked ', '');
  await page.getByRole('button', { name: 'Check again', exact: true }).click();
  await expect.poll(() => reads.length).toBeGreaterThanOrEqual(2);
  await expect(page.locator('.public-alert [data-tone="warning"]')).toBeVisible();
  await expect(badge).toHaveText("Didn't work");
  await expect(observation).toContainText('Updates are unavailable.');
  await expect(observation).toContainText(lastChecked);
  await page.screenshot({ path: info.outputPath('newer-progress-retained.png'), fullPage: true });
  recovered = true;
  await page
    .locator('.public-alert [data-tone="warning"]')
    .getByRole('button', { name: 'Check again', exact: true })
    .click();
  await expect(badge).toHaveText('Cancelled');
  await expect(page.locator('.public-alert [data-tone="warning"]')).toHaveCount(0);
  await expect(observation).not.toContainText('Updates are unavailable.');
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
  expect(streams).toBe(0);
  await info.attach('progress-observations', {
    contentType: 'application/json',
    body: JSON.stringify({ mocked: true, trigger: 'Check again', reads, streams, writes: 0 }),
  });
});
