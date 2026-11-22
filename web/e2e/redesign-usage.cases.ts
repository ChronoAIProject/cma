import { test, expect, type Locator } from '@playwright/test';
import { setupUsageRedesign } from './redesign-usage.fixture';

function reading(overview: Locator, label: string) {
  return overview.getByRole('term').filter({ hasText: label }).locator('..').locator('dd').first();
}

export function registerUsageRedesignCases() {
  for (const theme of ['dark', 'light']) {
    for (const width of [1440, 390]) {
      test(`LOCAL P5 Usage allocation explanation ${theme} ${width}`, async ({
        page,
        baseURL,
      }, info) => {
        const { boundary, metrics } = await setupUsageRedesign(page, baseURL!, { theme });
        await page.setViewportSize({ width, height: 1000 });
        await page.goto('/usage');
        const dialog = page.getByRole('dialog', { name: 'Your usage' });
        const overview = dialog.getByRole('region', { name: 'Allocation and history overview' });
        await expect(overview.getByRole('heading')).toHaveText('History not fully verified');
        await expect(dialog.getByText('Usage only, not a bill', { exact: true })).toBeVisible();
        await expect(reading(overview, 'Reported allocation')).toHaveText('41.5 core-h');
        await expect(reading(overview, 'Known gaps')).toHaveText('None confirmed');
        await expect(reading(overview, 'Projected tail')).toHaveText('2.0 h');
        await expect(overview).not.toContainText(/43.5|Confirmed 41.5|Never billed as fact/);
        await expect(dialog.getByRole('region', { name: 'Measured resource charts' })).toHaveCount(
          0,
        );
        const cards = overview.locator('.usage-allocation-overview__card');
        await expect(cards).toHaveCount(3);
        await expect
          .poll(() =>
            cards.evaluateAll(
              (nodes) =>
                new Set(nodes.map((node) => Math.round(node.getBoundingClientRect().y))).size,
            ),
          )
          .toBe(width === 1440 ? 1 : 3);
        const scroll = dialog.locator('.modal__body');
        expect(await scroll.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(
          true,
        );
        await expect(cards.first()).toHaveCSS(
          'background-color',
          theme === 'dark' ? 'rgb(20, 20, 22)' : 'rgb(255, 255, 255)',
        );
        await expect(cards.first()).toHaveCSS(
          'color',
          theme === 'dark' ? 'rgb(244, 244, 245)' : 'rgb(17, 17, 19)',
        );
        await scroll.evaluate((node) => node.scrollTo(0, 0));
        await page.screenshot({
          path: info.outputPath(`17-usage-${theme}-${width}.png`),
          animations: 'disabled',
        });
        await overview.scrollIntoViewIfNeeded();
        await overview.screenshot({
          path: info.outputPath(`17-usage-summary-${theme}-${width}.png`),
          animations: 'disabled',
        });
        const table = dialog.getByRole('table', { name: 'Sandboxes and allocated usage' });
        await table.scrollIntoViewIfNeeded();
        for (const name of ['Example environment', 'Research environment', 'Planning environment'])
          await expect(table.getByRole('button', { name, exact: true })).toBeVisible();
        await expect(table.getByText('Coverage unverified', { exact: true })).toHaveCount(1);
        await expect(table.getByText('Coverage verified', { exact: true })).toHaveCount(2);
        await page.screenshot({
          path: info.outputPath(`17-usage-environments-${theme}-${width}.png`),
          animations: 'disabled',
        });
        expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toHaveLength(0);
        expect(boundary.writes).toEqual([]);
        expect(boundary.errors).toEqual([]);
        expect(boundary.unexpected).toEqual([]);
      });
    }
  }

  test('LOCAL P5 Usage retains its background draft, focus return and measured permission', async ({
    page,
    baseURL,
  }) => {
    const { boundary, metrics } = await setupUsageRedesign(page, baseURL!, { measured: true });
    await page.goto('/agents/agt_contract');
    const editor = page.getByRole('textbox', { name: 'Message', exact: true });
    await editor.fill('Keep this background draft');
    const trigger = page.getByRole('button', { name: /Account menu for/ });
    await trigger.click();
    await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    await expect(dialog.getByText('Reported allocation', { exact: true })).toBeVisible();
    await expect(dialog.getByRole('region', { name: 'Measured resource charts' })).toBeVisible();
    await expect(dialog.getByRole('region', { name: 'Home PVC disk', exact: true })).toContainText(
      'Unavailable measured',
    );
    await expect(page.getByRole('dialog')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(dialog).not.toBeVisible();
    await expect(page).toHaveURL(/\/agents\/agt_contract$/);
    await expect(editor).toHaveValue('Keep this background draft');
    await expect(trigger).toBeFocused();
    expect(metrics.creates.every(({ admin }) => !admin)).toBe(true);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });

  test('LOCAL P5 Usage preserves reported totals through refresh backoff and replaces them on recovery', async ({
    page,
    baseURL,
  }) => {
    const now = Date.now();
    await page.clock.install({ time: now });
    const { boundary, metrics } = await setupUsageRedesign(page, baseURL!);
    let failure = false;
    let attempts = 0;
    await page.route('**/api/v1/usage/metrics/reports', (route) => {
      attempts += 1;
      return failure
        ? route.fulfill({
            status: 429,
            headers: { 'Retry-After': '30' },
            json: { status: 429, title: 'Reporting unavailable', code: 'usage_unavailable' },
          })
        : route.fallback();
    });
    await page.goto('/usage');
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    const overview = dialog.getByRole('region', { name: 'Allocation and history overview' });
    await expect(reading(overview, 'Reported allocation')).toHaveText('41.5 core-h');
    await page.clock.pauseAt(await page.evaluate(() => Date.now() + 100));
    failure = true;
    await dialog.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    await expect(dialog.getByText('Retrying usage automatically…', { exact: true })).toBeVisible();
    await expect(dialog.getByText(/As of.*refreshing/)).toBeVisible();
    await expect(reading(overview, 'Reported allocation')).toHaveText('41.5 core-h');
    await expect(reading(overview, 'Known gaps')).toHaveText('None confirmed');
    expect(attempts).toBe(2);
    await page.clock.fastForward(29999);
    expect(attempts).toBe(2);
    failure = false;
    metrics.allocatedPeriod = () => ({
      ...metrics.presentationRows![0]!.allocated_period,
      cpu_core_hours: 52,
      complete: false,
      unknown_seconds: 3600,
      projected_tail_seconds: 0,
    });
    await page.clock.fastForward(4000);
    await expect(reading(overview, 'Reported allocation')).toHaveText('52 core-h');
    await expect(reading(overview, 'Known gaps')).toHaveText('1h');
    await expect(reading(overview, 'Projected tail')).toHaveText('0.0 h');
    await expect(overview.getByRole('heading')).toHaveText('History has confirmed gaps');
    expect(attempts).toBe(3);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
