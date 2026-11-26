import { expect, test } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { environments, resourceRow } from './resource-lifecycle.fixture';

for (const purpose of ['image_rotation', 'revive'] as const) {
  test(`LOCAL parked ${purpose} is visible from the list and explicit retry preserves its intent`, async ({
    page,
    baseURL,
  }, info) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const boundary = new LocalStartup();
    boundary.sandbox = {
      ...boundary.sandbox,
      status: purpose === 'revive' ? 'reviving' : 'rotating',
      lifecycle: {
        pause: { allowed: false },
        revive: { allowed: false },
        operation: { purpose, phase: 'conversation_reopen', started_at: localTime },
        result: {
          state: 'failed',
          code: 'runtime_update_parked',
          continuing: false,
          operation_id: 'original-runtime-update',
          started_at: localTime,
          result_due_at: '2026-09-05T00:01:30Z',
          actions: ['observe'],
        },
      },
      runtime_update: {
        current_version: 'image-pinned',
        target_version: 'image-pinned',
        latest_version: 'image-newer',
        state: 'failed',
        can_update: true,
        update_available: true,
        preserves_home: true,
        schedule: { timezone: 'Asia/Singapore', local_time: '02:00' },
        failure: { code: 'terminal_validation', message: 'The runtime update stopped safely.' },
      },
    };
    await boundary.route(page, baseURL!);
    await page.route('**/api/v1/sandboxes', async (route) => {
      const { runtime_update: omitted, ...summary } = boundary.sandbox;
      void omitted;
      return route.fulfill({ json: { items: [summary] } });
    });
    const detailReads: number[] = [];
    let allowDetails = false;
    await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, (route) => {
      detailReads.push(Date.now());
      if (!allowDetails) {
        const { runtime_update: omitted, ...observation } = boundary.sandbox;
        void omitted;
        return route.fulfill({ json: observation });
      }
      return route.fallback();
    });
    const writes: { body: unknown; key?: string }[] = [];
    await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}/runtime-update`, async (route) => {
      const request = route.request();
      writes.push({ body: request.postDataJSON(), key: request.headers()['idempotency-key'] });
      if (writes.length === 1)
        return route.fulfill({
          status: 503,
          json: { code: 'unavailable', detail: 'The response is uncertain.' },
        });
      boundary.sandbox = {
        ...boundary.sandbox,
        revision: boundary.sandbox.revision + 1,
        runtime_update: {
          ...boundary.sandbox.runtime_update!,
          state: 'updating',
          can_update: false,
          failure: undefined,
        },
        lifecycle: {
          ...boundary.sandbox.lifecycle!,
          result: {
            ...boundary.sandbox.lifecycle!.result!,
            state: 'in_progress',
            code: undefined,
            continuing: true,
            retry_after_seconds: 5,
          },
        },
      };
      return route.fulfill({ status: 202, json: boundary.sandbox });
    });
    await page.goto('/agent-manager');
    await environments(page);
    const row = resourceRow(page, 'sandbox', boundary.sandbox.id);
    await expect(row).toContainText('Runtime update needs attention');
    await expect(
      row.locator('xpath=ancestor::*[contains(@class,"resource-tree__section")]').first(),
    ).toContainText('Needs attention');
    // Navigation already reads each hierarchy once. No extra status lookup,
    // cached Runtime panel or runtime_update field is available in this phase.
    expect(detailReads).toHaveLength(1);
    expect(writes).toEqual([]);
    await page.reload();
    await environments(page);
    await expect(resourceRow(page, 'sandbox', boundary.sandbox.id)).toContainText(
      'Runtime update needs attention',
    );
    allowDetails = true;
    await page.getByRole('button', { name: 'Tasks', exact: true }).click();
    await page.locator('.runtime-attention > summary').click();
    await resourceRow(page, 'agent', boundary.agent.id)
      .getByRole('button', { name: /^Options for/ })
      .click();
    await expect(page.getByRole('menuitem', { name: /^Stop\b/ })).toContainText(
      'Retry runtime update',
    );
    await page.getByRole('menuitem', { name: /^Retry runtime update/ }).click();
    await expect(page).toHaveURL(`${baseURL}/sandboxes/${boundary.sandbox.id}`);
    expect(writes).toEqual([]);
    await page.getByRole('tab', { name: 'Activity', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Sandbox progress' })).toContainText(
      'Last confirmed stage',
    );
    await expect(page.getByText('Preparation continues if you close this page.')).toHaveCount(0);
    await expect(page.getByText(/Resuming —|recovery is continuing/)).toHaveCount(0);
    await page.getByRole('button', { name: 'Review runtime update', exact: true }).click();
    const runtime = page.getByRole('region', { name: 'Runtime', exact: true });
    await expect(runtime).toBeVisible();
    expect(writes).toEqual([]);
    for (const dark of [true, false]) {
      await page.evaluate(
        (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
        dark,
      );
      await page.screenshot({
        path: info.outputPath(`${purpose}-${dark ? 'dark' : 'light'}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
    await runtime.getByRole('button', { name: 'Retry runtime update', exact: true }).click();
    await runtime.getByRole('button', { name: 'Retry update request', exact: true }).click();
    await expect.poll(() => writes.length).toBe(2);
    expect(writes[0]).toEqual(writes[1]);
    expect(writes[0]!.key).toBeTruthy();
    expect(writes[0]!.body).toEqual({ target_version: 'image-pinned' });
    await expect(page.getByRole('button', { name: 'Review runtime update' })).toHaveCount(0);
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'active',
      revision: boundary.sandbox.revision + 1,
      lifecycle: { pause: { allowed: true }, revive: { allowed: false } },
      runtime_update: {
        ...boundary.sandbox.runtime_update!,
        state: 'up_to_date',
        completed_at: localTime,
      },
    };
    await expect(page.getByRole('main').getByText('Live', { exact: true })).toBeVisible();
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
