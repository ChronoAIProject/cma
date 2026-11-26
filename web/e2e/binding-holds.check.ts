import { expect, test, type Page } from '@playwright/test';
import { heldBindings } from '../src/operator/bindingHolds.test-support';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';

const endpoint = '**/api/v1/admin/runtime-binding-holds';
const capabilities = ['use_own_sandboxes', 'read_all_sandboxes', 'read_activity_log'];

async function setup(
  page: Page,
  baseURL: string,
  advertisement: unknown,
  permissions = capabilities,
) {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL);
  await metrics.route(page);
  await new LocalUsageFilterOptions().route(page);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'local-owner',
        display_name: 'Local operator',
        visitor: false,
        capabilities: permissions,
        ...(advertisement === undefined ? {} : { binding_diagnostics: advertisement }),
        usage_reporting: { measured_visible: false },
      },
    }),
  );
  await page.route('**/api/v1/admin/activity**', (route) =>
    route.fulfill({ json: { items: [], next: null } }),
  );
  return boundary;
}

function emptySample() {
  return { ...heldBindings(), observed_sandboxes: 0, reports: [], unavailable_reporters: 3 };
}

for (const theme of ['light', 'dark']) {
  test(`LOCAL held bindings stay readable and read-only in ${theme}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = await setup(page, baseURL!, true);
    const sample = heldBindings();
    const sandbox = `sbx_${'observed_long_sandbox_'.repeat(14)}`;
    const owner = 'Alex Chen with a long operator-visible display name '.repeat(5).trim();
    const long = {
      ...sample,
      truncated: true,
      reports: sample.reports.map((report) => ({
        ...report,
        sample: {
          ...report.sample,
          holds: report.sample.holds.map((hold) => ({
            ...hold,
            sandbox_id: sandbox,
            owner_name: hold.owner_name ? owner : null,
          })),
        },
      })),
    };
    const reads: string[] = [];
    await page.route(endpoint, (route) => {
      reads.push(route.request().method());
      expect(route.request().postData()).toBeNull();
      return route.fulfill({ json: reads.length === 1 ? long : emptySample() });
    });
    await page.addInitScript(
      (appearance) => localStorage.setItem('cma:appearance', appearance),
      theme,
    );
    await page.goto('/operations?tab=sandboxes');
    const panel = page.getByRole('region', { name: 'Deferred runtime bindings' });
    await expect(panel.getByText('1 observed Sandboxes')).toBeVisible();
    await expect(
      panel.getByRole('heading', { name: `Sandbox ${sandbox}`, exact: true }),
    ).toHaveCount(1);
    await expect(panel.getByText(owner, { exact: true })).toBeVisible();
    await expect(panel.getByText('Name unavailable', { exact: true })).toBeVisible();
    await expect(panel.getByText('private-owner-id', { exact: true })).toHaveCount(0);
    await expect(panel.getByText('Retrying at observation time', { exact: true })).toBeVisible();
    await expect(panel.getByText(/Eligible no earlier than/)).toBeVisible();
    await expect(panel.getByText(/Incomplete observation/)).toBeVisible();
    await expect(panel.getByText(/does not establish deployment-wide health/)).toBeVisible();
    const button = panel.getByRole('button', { name: 'Refresh binding observations' });
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.locator('.operator-scroll').evaluate((element) => element.scrollTo(0, 0));
      await expect
        .poll(async () =>
          panel.locator('h2, h3, h4, dt, dd, button').evaluateAll((elements) =>
            elements
              .map((element) => {
                const rect = element.getBoundingClientRect();
                return { left: rect.left, right: rect.right, width: window.innerWidth };
              })
              .filter((rect) => rect.left < -1 || rect.right > rect.width + 1),
          ),
        )
        .toEqual([]);
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      const target = await button.boundingBox();
      expect(target!.height).toBeGreaterThanOrEqual(44);
      expect(target!.width).toBeGreaterThanOrEqual(44);
      await page.screenshot({
        path: info.outputPath(`binding-holds-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
    await button.focus();
    await expect(button).toBeFocused();
    await page.keyboard.press('Enter');
    await expect(panel.getByText('0 observed Sandboxes')).toBeVisible();
    await expect(
      panel.getByText('No held bindings were observed in the returned samples.'),
    ).toBeVisible();
    await expect(panel.getByText(/Incomplete observation/)).toBeVisible();
    await expect(panel.getByText(owner, { exact: true })).toHaveCount(0);
    await expect(panel.getByText(/all.*healthy|recovered successfully/i)).toHaveCount(0);
    expect(reads).toEqual(['GET', 'GET']);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

for (const advertisement of [undefined, false, 'true']) {
  test(`LOCAL held bindings make no request for advertisement ${String(advertisement)}`, async ({
    page,
    baseURL,
  }) => {
    const boundary = await setup(page, baseURL!, advertisement);
    let reads = 0;
    await page.route(endpoint, (route) => {
      reads += 1;
      return route.fulfill({ json: heldBindings() });
    });
    await page.goto('/operations?tab=sandboxes');
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Deferred runtime bindings' })).toHaveCount(0);
    expect(reads).toBe(0);
    expect(boundary.writes).toEqual([]);
  });
}

test('LOCAL advertised diagnostics still require cross-owner read permission', async ({
  page,
  baseURL,
}) => {
  const boundary = await setup(page, baseURL!, true, ['use_own_sandboxes', 'read_activity_log']);
  let reads = 0;
  await page.route(endpoint, (route) => {
    reads += 1;
    return route.fulfill({ json: heldBindings() });
  });
  await page.goto('/operations?tab=activity');
  await expect(page.getByRole('tab', { name: 'Activity', exact: true })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Deferred runtime bindings' })).toHaveCount(0);
  expect(reads).toBe(0);
  expect(boundary.writes).toEqual([]);
});

test('LOCAL binding refresh clears rejected rows and honors Retry-After without polling', async ({
  page,
  baseURL,
}) => {
  const boundary = await setup(page, baseURL!, true);
  const reads: number[] = [];
  await page.route(endpoint, (route) => {
    reads.push(Date.now());
    expect(route.request().method()).toBe('GET');
    if (reads.length === 2)
      return route.fulfill({
        status: 503,
        headers: { 'Retry-After': '1' },
        json: { status: 503, title: 'Unavailable', detail: 'private raw service failure' },
      });
    return route.fulfill({ json: reads.length === 1 ? heldBindings() : emptySample() });
  });
  await page.goto('/operations?tab=sandboxes');
  const panel = page.getByRole('region', { name: 'Deferred runtime bindings' });
  await expect(panel.getByText('Alex Chen', { exact: true })).toBeVisible();
  const button = panel.getByRole('button', { name: 'Refresh binding observations' });
  await button.click();
  await expect(panel.getByRole('alert')).toContainText(
    'Runtime binding observations are unavailable.',
  );
  await expect(panel.getByText('Alex Chen', { exact: true })).toHaveCount(0);
  await expect(panel.getByText('private raw service failure', { exact: true })).toHaveCount(0);
  await expect(button).toBeDisabled();
  expect(reads).toHaveLength(2);
  await expect(button).toBeEnabled({ timeout: 5000 });
  expect(reads).toHaveLength(2);
  await button.click();
  await expect(panel.getByText('0 observed Sandboxes')).toBeVisible();
  expect(reads).toHaveLength(3);
  expect(reads[2]! - reads[1]!).toBeGreaterThanOrEqual(1000);
  expect(boundary.writes).toEqual([]);
});

test('LOCAL leaving binding observations rejects a late protected response', async ({
  page,
  baseURL,
}) => {
  const boundary = await setup(page, baseURL!, true);
  let release!: () => void;
  const released = new Promise<void>((resolve) => {
    release = resolve;
  });
  let requested = false;
  let replied = false;
  await page.route(endpoint, async (route) => {
    requested = true;
    await released;
    await route.fulfill({ json: heldBindings() });
    replied = true;
  });
  await page.goto('/operations?tab=sandboxes');
  await expect.poll(() => requested).toBe(true);
  await expect(page.getByText('Reading binding observations…', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: 'Activity', exact: true }).click();
  release();
  await expect.poll(() => replied).toBe(true);
  await expect(page.getByRole('region', { name: 'Deferred runtime bindings' })).toHaveCount(0);
  await expect(page.getByText('Alex Chen', { exact: true })).toHaveCount(0);
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
