import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL browser UI decodes bounded images, defaults selection, expires Live and preserves the conversation', async ({
  page,
  baseURL,
}, info) => {
  const proof = process.env.CMA_BROWSER_PROOF_DIRECTORY;
  if (!proof)
    throw new Error(
      'Explicit native development image directory required for this U protocol check',
    );
  const images = [0, 1].map((index) =>
    readFileSync(join(proof, `native-page-${index}.jpg`)).toString('base64'),
  );
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const feature = (state: string) => ({
    state,
    reason: 'controlled_U_protocol',
    scope: 'agent_browser',
    writable: false,
  });
  await page.route('**/api/v1/agents/agt_contract/environment', (route) =>
    route.fulfill({
      json: {
        agent_id: 'agt_contract',
        policy_revision: 1,
        runtime_incarnation: 'ui-proof',
        revalidate_at: null,
        files: feature('available'),
        operation_output: feature('available'),
        browser: feature('available'),
        replay: feature('unsupported'),
        control: feature('unsupported'),
      },
    }),
  );
  let current = 0;
  let sequence = 0;
  let disconnected = false;
  let reads = 0;
  let releases = 0;
  await page.route('**/api/v1/agents/agt_contract/browser?*', (route) => {
    if (route.request().method() === 'DELETE') releases += 1;
    else reads += 1;
    if (disconnected && route.request().method() === 'GET') return route.abort();
    return route.fulfill({
      json: {
        agent_id: 'agt_contract',
        runtime_incarnation: 'ui-proof',
        context_id: 'e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24',
        state: 'live',
        location: 'Local page',
        observers: 1,
        captures: sequence,
        bytes: 20000,
        min_interval_ms: 1000,
        fresh_for_ms: 3000,
        max_frame_bytes: 262144,
        max_observers: 16,
        frame: {
          sequence: ++sequence,
          captured_at: new Date().toISOString(),
          media_type: 'image/jpeg',
          width: 1280,
          height: 720,
          data: images[current],
        },
      },
    });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  await page.getByRole('textbox', { name: 'Message' }).fill('Keep this conversation draft');
  if (await page.getByRole('button', { name: 'Hide Agent computer' }).isVisible())
    await page.getByRole('button', { name: 'Back to conversation' }).click();
  await page.getByRole('button', { name: 'Show Agent computer' }).click();
  await expect(page.getByRole('tab', { name: 'Browser', exact: true })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(page.getByText('Live', { exact: true })).toBeVisible();
  const image = page.getByAltText('Agent browser page');
  await expect(image).toHaveJSProperty('naturalWidth', 1280);
  await expect(image).toHaveAttribute('src', `data:image/jpeg;base64,${images[0]}`);
  current = 1;
  await expect(image).toHaveAttribute('src', `data:image/jpeg;base64,${images[1]}`);
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    await expect(image).toBeVisible();
    await expect
      .poll(() =>
        image.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          return (
            bounds.left >= 0 &&
            bounds.right <= innerWidth &&
            bounds.top >= 0 &&
            bounds.bottom <= innerHeight
          );
        }),
      )
      .toBe(true);
    await page.screenshot({ path: info.outputPath(`browser-${width}.png`), fullPage: true });
  }
  disconnected = true;
  await expect(page.getByText('Live', { exact: true })).not.toBeVisible({ timeout: 5000 });
  await expect(page.getByText('Last frame · Reconnecting…')).toBeVisible();
  await page.getByRole('button', { name: 'Back to conversation' }).click();
  await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue(
    'Keep this conversation draft',
  );
  await expect.poll(() => releases).toBeGreaterThan(0);
  const closed = reads;
  await page.waitForTimeout(1500);
  expect(reads).toBe(closed);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
