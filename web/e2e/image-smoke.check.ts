import { test, expect } from '@playwright/test';
import type { V2UiTransport } from '@cma/client';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalRetainedUsageMetrics } from './usage-retained.fixture';

// Use the image's real nginx headers. Production returned no CSP on 2026-11-06;
// Vite preview cannot exercise the retained-volume overlay or nginx fallback.
for (const admin of [false, true]) {
  for (const theme of ['light', 'dark']) {
    test(`IMAGE with stale assets: ${admin ? 'operations' : 'personal'} ${theme}`, async ({
      page,
      baseURL,
      request,
    }, info) => {
      const settledAssets = async () => {
        await expect(page.locator('img.sidebar__mark')).not.toHaveJSProperty('naturalWidth', 0);
        await expect(page.locator('.character-avatar[data-status="loading"]')).toHaveCount(0);
        await page.evaluate(() => window.document.fonts.ready.then(() => undefined));
      };
      const boundary = new LocalStartup();
      const metrics = new LocalRetainedUsageMetrics();
      const errors: string[] = [];
      const failedAssets: string[] = [];
      page.on('console', (message) => {
        if (message.type() !== 'error') return;
        // The retained image fixture represents a replica that predates the
        // bounded initial-view routes. The client deliberately falls back to
        // the legacy report path for this explicit 501 route absence, which
        // Chromium still reports as a console error. Keep that expected
        // compatibility signal out of the stale-asset error assertion while
        // retaining all other network and application errors.
        const location = message.location().url;
        if (
          location.includes('/api/v1/') &&
          location.includes('/usage/metrics/initial/') &&
          message.text().includes('status of 501')
        )
          return;
        errors.push(`${location}: ${message.text()}`);
      });
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('response', (response) => {
        if (new URL(response.url()).pathname.startsWith('/assets/') && !response.ok())
          failedAssets.push(`${response.status()} ${new URL(response.url()).pathname}`);
      });
      await boundary.route(page, baseURL!);
      await page.route('**/api/v1/agents/agt_contract/reply-preview', (route) => {
        if (route.request().method() !== 'GET') return route.fallback();
        return route.fulfill({
          json: {
            status: 'none',
            agent_id: 'agt_contract',
            source_through_seq: 0,
            latest_assistant_reply: null,
          },
        });
      });
      await metrics.route(page);
      await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
      let transportSelections = 0;
      // List prefetch asks for eligibility. Model a current disabled server here;
      // LocalStartup retains optional-endpoint 404s for legacy compatibility tests.
      await page.route('**/api/v2/ui/transport', (route) => {
        expect(route.request().method()).toBe('GET');
        transportSelections += 1;
        return route.fulfill({
          json: {
            object: 'cma.ui.transport',
            schema_version: 1,
            enabled: false,
            selection: 'new_tabs',
            input: 'responses_v2',
            snapshot_schema: 1,
            event_schema: 1,
            control_schema: 1,
            live_fragments: true,
            snapshot: '/api/v2/agents/{agent_id}/events/snapshot',
            events: '/api/v2/agents/{agent_id}/events',
            controls: '/api/v2/agents/{agent_id}/responses/{response_id}/controls',
            command_status: '/api/v2/commands/{command_id}',
          } satisfies V2UiTransport,
        });
      });
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
      // Re-execute the bundle, then revisit through browser history with the same assets mount.
      await settledAssets();
      await page.reload();
      await expect(surface).toBeVisible();
      await expect(
        surface.getByRole('button', { name: 'Research Sandbox', exact: true }),
      ).toBeVisible();
      await settledAssets();
      await page.goto('/agent-manager');
      await expect(page.getByRole('button', { name: /Account menu for/ })).toBeVisible();
      await settledAssets();
      await page.goBack();
      await expect(surface).toBeVisible();
      await expect(
        surface.getByRole('button', { name: 'Research Sandbox', exact: true }),
      ).toBeVisible();
      await settledAssets();
      await page.screenshot({ path: info.outputPath('overview.png'), animations: 'disabled' });
      expect(failedAssets).toEqual([]);
      expect(errors).toEqual([]);
      expect(transportSelections).toBeGreaterThan(0);
      expect(metrics.creates).toEqual([]);
      expect(metrics.views.length).toBeGreaterThan(0);
      expect(metrics.views.every((url) => url.pathname.includes('/admin/') === admin)).toBe(true);
      for (const url of metrics.views) {
        expect(url.searchParams.has('step_seconds')).toBe(false);
        expect(url.searchParams.get('refresh')).toBe('false');
      }
      expect(metrics.reads.some((url) => url.pathname.includes('/reports/retained-'))).toBe(true);
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
