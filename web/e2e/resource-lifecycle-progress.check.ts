import { test, expect } from '@playwright/test';
import type { Launch } from '../src/api-client/index';
import { LocalStartup, recorded } from './agent-startup-fixtures';

for (const width of [1440, 390]) {
  for (const surface of ['launch', 'sandbox'] as const) {
    test(`LOCAL live restore progress on ${surface} at ${width}px`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      const startedAt = new Date(Date.now() - 185_000).toISOString();
      const operation = {
        purpose: 'revive' as const,
        phase: 'home_restore' as const,
        started_at: startedAt,
      };
      boundary.sandbox = {
        ...boundary.sandbox,
        status: 'reviving',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false }, operation },
      };
      await boundary.route(page, baseURL!);
      const original = recorded<Launch>('launches-state.json', 'launch-state-repeat-new');
      const record: Launch = {
        ...original,
        steps: original.steps.map((step) =>
          step.key === 'sandbox' ? { ...step, status: 'running' } : step,
        ),
        state: 'sandbox_pending',
        blocked_reason: { type: 'sandbox_paused_reviving' },
        resolution: {
          sandbox: { state: 'reused', id: boundary.sandbox.id },
          workspace: { state: 'pending' },
          agent: { state: 'pending' },
          choice_required: false,
        },
      };
      await page.route('**/api/v1/launches/lch_contract', (route) => {
        expect(route.request().method()).toBe('GET');
        return route.fulfill({ json: record });
      });
      await page.route('**/api/v1/agui/launches/**', (route) => {
        expect(route.request().method()).toBe('GET');
        return route.fulfill({ status: 503, json: { title: 'Stream unavailable' } });
      });
      await page.goto(
        surface === 'launch' ? '/launches/lch_contract' : `/sandboxes/${boundary.sandbox.id}`,
      );
      if (surface === 'sandbox')
        await page.getByRole('button', { name: 'View progress', exact: true }).click();
      const progress = page.getByRole('region', { name: 'Sandbox progress' });
      await expect(progress).toBeVisible();
      await expect(progress.getByText(/Elapsed: 3m/)).toBeVisible();
      await expect(
        progress.getByText(/Checking your files and installed software before your agents reopen./),
      ).toBeVisible();
      await progress.getByText('Activity details').click();
      await expect(progress.getByText('Done', { exact: true })).toHaveCount(3);
      for (const dark of [false, true]) {
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
          dark,
        );
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await page.screenshot({
          path: info.outputPath(`restore-${surface}-${width}-${dark ? 'dark' : 'light'}.png`),
          fullPage: true,
        });
      }
      let failReads = true;
      await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, (route) => {
        expect(route.request().method()).toBe('GET');
        return failReads
          ? route.fulfill({ status: 503, json: { title: 'Temporarily unavailable' } })
          : route.fulfill({ json: boundary.sandbox });
      });
      await expect(progress.getByText(/Status updates interrupted/)).toBeVisible({
        timeout: 10_000,
      });
      await expect(progress.getByText('Last confirmed stage', { exact: true })).toBeVisible();
      failReads = false;
      boundary.sandbox = {
        ...boundary.sandbox,
        lifecycle: {
          ...boundary.sandbox.lifecycle!,
          operation: { ...operation, phase: 'conversation_reopen' },
        },
      };
      await expect(progress.getByText('Done', { exact: true })).toHaveCount(4, { timeout: 10_000 });
      await expect(progress).toContainText('Reopening');
      await expect(progress.getByText(/Status updates interrupted/)).toHaveCount(0);
      await page.reload();
      await expect(progress).toBeVisible();
      await expect(progress.getByText(/Elapsed: 3m/)).toBeVisible();
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
