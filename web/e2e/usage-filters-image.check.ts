import { writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions, filterOwners } from './usage-filter-options.fixture';

for (const theme of ['light', 'dark']) {
  test(`IMAGE Ops dropdowns, fleet search, report changes and failures ${theme}`, async ({
    page,
    baseURL,
    request,
  }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    const options = new LocalUsageFilterOptions();
    metrics.summaryCount = (input) => options.matching(input).length;
    metrics.runningCount = (input) =>
      options.matching(input).filter((row) => row.status === 'active').length;
    metrics.pending = true;
    const assets: { path: string; status: number }[] = [];
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('response', (response) => {
      if (new URL(response.url()).pathname.startsWith('/assets/'))
        assets.push({ path: new URL(response.url()).pathname, status: response.status() });
    });
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await options.route(page);
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
    const document = await page.goto('/operations?tab=sandboxes');
    expect(document?.headers()['cache-control']).toBe('no-store');
    const filters = page.getByRole('form', { name: 'Usage filters' });
    await expect(filters).toBeVisible();
    await expect(filters.locator('input')).toHaveCount(0);
    const apply = async () => {
      const before = metrics.creates.length;
      await page.getByRole('button', { name: 'Apply usage filters' }).click();
      await expect.poll(() => metrics.creates.length).toBe(before + 1);
      await expect(
        page.getByRole('button', { name: 'Research Sandbox', exact: true }),
      ).toBeVisible();
      const id = metrics.creates.at(-1)!.id;
      await expect
        .poll(() => metrics.reads.filter((url) => url.pathname.endsWith(`/reports/${id}`)).length)
        .toBe(1);
      for (const created of metrics.creates)
        expect(
          metrics.reads.filter((url) => url.pathname.endsWith(`/reports/${created.id}`)).length,
        ).toBeLessThanOrEqual(1);
    };
    // Open and search while reporting is still pending. Discovery never submits a report.
    await page.getByRole('button', { name: 'Owner: Everyone' }).click();
    await page.getByRole('textbox', { name: 'Search owner choices' }).fill('Alex');
    await expect(page.getByRole('menuitemradio', { name: /Alex Chen/ })).toHaveCount(2);
    expect(metrics.creates).toHaveLength(1);
    await expect(
      page.getByRole('menuitemradio', { name: 'Alex Chen Joined 2026-11-01', exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole('menuitemradio', { name: 'Unnamed owner', exact: true }),
    ).toHaveCount(0);
    expect(await page.getByRole('menu').innerText()).not.toMatch(
      /account [0-9a-f]{12}|reference [0-9a-f]{12}/,
    );
    await page.screenshot({
      path: info.outputPath(`owner-search-${theme}.png`),
      animations: 'disabled',
    });
    await page
      .getByRole('menuitemradio', { name: /Alex Chen/ })
      .first()
      .click();
    await expect(
      page.getByRole('button', { name: 'Owner: Alex Chen', exact: true }),
    ).toHaveAttribute('title', 'Alex Chen');
    await apply();
    expect(metrics.creates.at(-1)?.input.filters?.owner).toBe(filterOwners[0]!.value);
    await page.screenshot({
      path: info.outputPath(`owner-selected-${theme}.png`),
      animations: 'disabled',
    });
    await page.getByRole('button', { name: 'Sandbox: All Sandboxes' }).click();
    await expect(page.getByRole('menuitemradio')).toHaveCount(51);
    expect(options.reads.at(-1)?.searchParams.get('owner')).toBe(filterOwners[0]!.value);
    await page.getByRole('menu').evaluate((element) => element.scrollTo(0, element.scrollHeight));
    await page.getByRole('button', { name: 'Next choices' }).click();
    await expect(page.getByRole('menuitemradio', { name: /Research 00150/ })).toBeVisible();
    await expect(page.getByRole('menuitemradio')).toHaveCount(51);
    await page.getByRole('textbox', { name: 'Search sandbox choices' }).fill('Research 19998');
    await expect(page.getByRole('menuitemradio', { name: /Research 19998/ })).toHaveCount(1);
    await expect(
      page.getByRole('menuitemradio', { name: 'Research 19998', exact: true }),
    ).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`sandbox-search-${theme}.png`),
      animations: 'disabled',
    });
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    await apply();
    expect(metrics.creates.at(-1)?.input.filters?.sandbox_id).toBe(options.rows[19998]!.value);
    await expect(
      page.getByRole('button', { name: 'Sandbox: Research 19998', exact: true }),
    ).toHaveAttribute('title', 'Research 19998');
    await expect(page.getByText(/^1 Sandboxes · 1 running$/)).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`sandbox-selected-${theme}.png`),
      animations: 'disabled',
    });
    await page.getByRole('button', { name: /^Sandbox: Research/ }).click();
    await page.getByRole('menuitemradio', { name: 'All Sandboxes' }).click();
    await apply();
    expect(metrics.creates.at(-1)?.input.filters?.sandbox_id).toBeUndefined();
    await page.getByRole('button', { name: /^Owner: Alex Chen/ }).click();
    await page.getByRole('menuitemradio', { name: 'Everyone' }).click();
    await apply();
    expect(metrics.creates.at(-1)?.input.filters?.owner).toBeUndefined();
    for (const [control, search, choice, field, value] of [
      [
        'Runtime version: Any runtime',
        'Search runtime version choices',
        'runtime-stable',
        'runtime_version',
        'runtime-stable',
      ],
      ['Usage Sandbox status', 'Search usage sandbox status', 'active', 'statuses', 'active'],
      ['Usage time range', 'Search usage time range', 'Last 7 days', 'range', '7d'],
    ] as const) {
      await page.getByRole('button', { name: control, exact: true }).click();
      await page.getByRole('textbox', { name: search }).fill(choice);
      await expect(page.getByRole('menuitemradio', { name: choice, exact: true })).toBeVisible();
      await page.screenshot({
        path: info.outputPath(`${field}-dropdown-${theme}.png`),
        animations: 'disabled',
      });
      await page.getByRole('menuitemradio', { name: choice, exact: true }).click();
      await apply();
      const current = metrics.creates.at(-1)!.input;
      if (field === 'range') expect(current.to! - current.from!).toBe(604800);
      else if (field === 'statuses') expect(current.filters?.statuses).toEqual([value]);
      else expect(current.filters?.runtime_version).toBe(value);
    }
    await page.getByRole('button', { name: 'Usage time range' }).click();
    await page.getByRole('menuitemradio', { name: 'Custom (up to 30 days)' }).click();
    await page.getByLabel('Usage range from').fill('2026-11-01T00:00');
    await page.getByLabel('Usage range to').fill('2026-11-02T00:00');
    await apply();
    await page.screenshot({ path: info.outputPath(`custom-and-filtered-${theme}.png`) });
    await page.getByRole('button', { name: 'Clear filters' }).click();
    await expect(page.getByRole('button', { name: 'Owner: Everyone' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Runtime version: Any runtime' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Usage time range' })).toContainText(
      'Last 24 hours',
    );
    await expect(page.getByRole('button', { name: 'Usage Sandbox status' })).toContainText(
      'Any status',
    );
    await expect.poll(() => metrics.creates.at(-1)?.input.filters?.statuses).toEqual([]);
    for (const [control, choice] of [
      ['Allocation chart resource', 'Memory'],
      ['Allocation chart resource', 'CPU'],
      ['Allocation chart grouping', 'By owner'],
      ['Allocation chart grouping', 'By Sandbox'],
    ]) {
      await page.getByRole('button', { name: control!, exact: true }).click();
      await page.getByRole('menuitemradio', { name: choice!, exact: true }).click();
      await expect(page.getByRole('button', { name: control!, exact: true })).toContainText(
        choice!,
      );
    }
    options.failure = 503;
    await page.getByRole('button', { name: 'Owner: Everyone' }).click();
    await expect(
      page.getByRole('alert').filter({ hasText: 'Could not load owner choices' }),
    ).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`option-error-${theme}.png`),
      animations: 'disabled',
    });
    options.failure = 0;
    await page.getByRole('button', { name: 'Retry choices' }).click();
    await expect(page.getByRole('menuitemradio', { name: /Unnamed owner/ })).toBeVisible();
    await page.getByRole('textbox', { name: 'Search owner choices' }).fill('Nobody matches');
    await expect(page.getByText('No matching owner choices.')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('button', { name: 'Owner: Everyone' })).toBeFocused();
    await expect(page.getByRole('menu', { includeHidden: true })).toHaveCount(0);
    expect(await filters.innerText()).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}-|sbx_/);
    await page.screenshot({ path: info.outputPath(`overview-${theme}.png`), fullPage: true });
    expect(errors).toEqual([]);
    expect(assets.length).toBeGreaterThan(1);
    expect(assets.every((asset) => asset.status === 200)).toBe(true);
    const html = await (await request.get('/')).text();
    for (const match of html.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g))
      expect((await request.get(match[1]!)).status()).toBe(200);
    const evidence = info.outputPath('image-entry-assets.json');
    writeFileSync(
      evidence,
      JSON.stringify(
        {
          theme,
          headers: document?.headers(),
          assets,
          reportCreates: metrics.creates,
          optionRequests: options.reads.map(String),
        },
        null,
        2,
      ),
    );
    await info.attach('image-entry-assets', { path: evidence, contentType: 'application/json' });
    expect(boundary.writes).toEqual([]);
  });
}
