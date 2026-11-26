import { test, expect, type Locator } from '@playwright/test';
import { setupUsageRedesign } from './redesign-usage.fixture';

function reading(overview: Locator, label: string) {
  return overview.getByRole('term').filter({ hasText: label }).locator('..').locator('dd').first();
}

export function registerUsageRedesignCases() {
  registerUsageLayoutCases();
  for (const theme of ['dark', 'light']) {
    for (const width of [1440, 390]) {
      test(`LOCAL P5 Usage allocation explanation ${theme} ${width}`, async ({
        page,
        baseURL,
      }, info) => {
        const { boundary, metrics } = await setupUsageRedesign(page, baseURL!, { theme });
        await page.setViewportSize({ width, height: width === 1440 ? 900 : 844 });
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

function registerUsageLayoutCases() {
  const sizes = [
    { name: 'desktop', width: 1440, height: 900, scale: 1 },
    { name: 'mobile', width: 390, height: 844, scale: 1 },
    // Layout/DPR emulation of 200% zoom, not the browser's native zoom control.
    { name: '200% reflow equivalent', width: 720, height: 450, scale: 2 },
  ];
  for (const theme of ['dark', 'light']) {
    for (const size of sizes) {
      test.describe(`LOCAL P5 Usage ${theme} ${size.name}`, () => {
        test.use({
          viewport: { width: size.width, height: size.height },
          deviceScaleFactor: size.scale,
        });
        test('keeps long names readable and keyboard controls reachable with reduced motion', async ({
          page,
          baseURL,
        }, info) => {
          const { boundary, metrics } = await setupUsageRedesign(page, baseURL!, { theme });
          await page.emulateMedia({ reducedMotion: 'reduce' });
          const name = 'Long environment name for a retained research workspace '.repeat(4).trim();
          metrics.presentationRows = metrics.presentationRows!.map((row, index) =>
            index === 0 ? { ...row, name } : row,
          );
          await page.goto('/agents/agt_contract');
          const editor = page.getByRole('textbox', { name: 'Message', exact: true });
          await editor.fill('Keep this draft through Usage keyboard navigation');
          if (size.name === 'mobile') {
            await page.getByRole('button', { name: 'Show navigation', exact: true }).focus();
            await page.keyboard.press('Enter');
          }
          const trigger = page.getByRole('button', { name: /Account menu for/ });
          await trigger.focus();
          await page.keyboard.press('Enter');
          await page.getByRole('menuitem', { name: 'Usage', exact: true }).focus();
          await page.keyboard.press('Enter');
          const dialog = page.getByRole('dialog', { name: 'Your usage' });
          const overview = dialog.getByRole('region', { name: 'Allocation and history overview' });
          await expect(reading(overview, 'Reported allocation')).toHaveText('41.5 core-h');
          const geometry = await dialog.evaluate((node) => {
            const body = node.querySelector('.modal__body')!;
            const cards = [...node.querySelectorAll('.usage-allocation-overview__card')];
            return {
              viewport: { width: innerWidth, height: innerHeight, dpr: devicePixelRatio },
              reducedMotion: matchMedia('(prefers-reduced-motion: reduce)').matches,
              bodyOverflow: body.scrollWidth > body.clientWidth + 1,
              pageOverflow: document.documentElement.scrollWidth > innerWidth + 1,
              cardOverflow: cards.some((card) => card.scrollWidth > card.clientWidth + 1),
              rows: new Set(cards.map((card) => Math.round(card.getBoundingClientRect().y))).size,
            };
          });
          expect(geometry).toEqual({
            viewport: { width: size.width, height: size.height, dpr: size.scale },
            reducedMotion: true,
            bodyOverflow: false,
            pageOverflow: false,
            cardOverflow: false,
            rows: size.name === 'mobile' ? 3 : 1,
          });
          await info.attach('layout', {
            body: JSON.stringify(geometry),
            contentType: 'application/json',
          });
          await page.screenshot({
            path: info.outputPath('17-usage-viewport.png'),
            animations: 'disabled',
          });
          await overview.screenshot({
            path: info.outputPath('17-usage-overview.png'),
            animations: 'disabled',
          });
          const table = dialog.getByRole('table', { name: 'Sandboxes and allocated usage' });
          const environment = table.getByRole('button', { name, exact: true });
          await expect(environment).toHaveAttribute('title', name);
          await environment.scrollIntoViewIfNeeded();
          expect(await table.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(
            true,
          );
          await page.screenshot({
            path: info.outputPath('17-usage-long-environment.png'),
            animations: 'disabled',
          });
          const refresh = dialog.getByRole('button', { name: 'Refresh usage', exact: true });
          let reachedRefresh = false;
          for (let index = 0; index < 24; index += 1) {
            await page.keyboard.press('Tab');
            expect(await dialog.evaluate((node) => node.contains(document.activeElement))).toBe(
              true,
            );
            if (await refresh.evaluate((node) => node === document.activeElement)) {
              reachedRefresh = true;
              break;
            }
          }
          expect(reachedRefresh).toBe(true);
          await expect(refresh).toBeInViewport();
          await page.keyboard.press('Escape');
          await expect(dialog).not.toBeVisible();
          await expect(page).toHaveURL(/\/agents\/agt_contract$/);
          await expect(trigger).toBeFocused();
          await expect(editor).toHaveValue('Keep this draft through Usage keyboard navigation');
          expect(boundary.writes).toEqual([]);
          expect(boundary.unexpected).toEqual([]);
          expect(boundary.errors).toEqual([]);
        });
      });
    }
  }
}
