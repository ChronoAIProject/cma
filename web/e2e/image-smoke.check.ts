import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';

// Use the image's real nginx headers. Production returned no CSP on 2026-11-06;
// Vite preview cannot exercise the retained-volume overlay or nginx fallback.
for (const admin of [false, true]) {
  for (const theme of ['light', 'dark']) {
    test(`IMAGE with stale assets: ${admin ? 'operations' : 'personal'} ${theme}`, async ({
      page,
      baseURL,
      request,
    }, info) => {
      const boundary = new LocalStartup();
      const metrics = new LocalUsageMetrics();
      const errors: string[] = [];
      const failedAssets: string[] = [];
      page.on('console', (message) => {
        if (message.type() === 'error') errors.push(`${message.location().url}: ${message.text()}`);
      });
      page.on('response', (response) => {
        if (new URL(response.url()).pathname.startsWith('/assets/') && !response.ok())
          failedAssets.push(`${response.status()} ${new URL(response.url()).pathname}`);
      });
      await boundary.route(page, baseURL!);
      await metrics.route(page);
      await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
      await page.route('**/api/v2/ui/task-experience', (route) =>
        route.fulfill({
          json: {
            object: 'cma.ui.task_experience',
            schema_version: 1,
            account_id: 'local-owner',
            capabilities: {
              task_home_admission: false,
              suggested_followup_admission: false,
              desktop_enable: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const document = await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
      expect(document?.headers()['cache-control']).toBe('no-store');
      const surface = admin
        ? page.getByRole('region', { name: 'Sandbox resource dashboard' })
        : page.getByRole('dialog', { name: 'Your usage' });
      await expect(surface).toBeVisible();
      await expect(
        surface
          .getByRole('region', { name: 'Allocation comparison chart' })
          .locator('.recharts-bar-rectangle')
          .first(),
      ).toBeVisible();
      await page.screenshot({ path: info.outputPath('overview.png'), animations: 'disabled' });
      expect(failedAssets).toEqual([]);
      expect(errors).toEqual([]);
      expect(boundary.errors).toEqual([]);
      expect(boundary.writes).toEqual([]);
      const retained = await request.get('/assets/retained-smoke.js');
      expect(retained.status()).toBe(200);
      expect(await retained.text()).toBe('/* retained release */\n');
      expect(retained.headers()['cache-control']).toContain('immutable');
      const missing = await request.get('/assets/missing-smoke.js');
      expect(missing.status()).toBe(404);
      expect(missing.headers()['cache-control']).toBe('no-store');
    });
  }
}
