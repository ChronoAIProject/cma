import { expect, test } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

for (const uncertain of [false, true]) {
  test(`LOCAL owner deletes a parked runtime update with uncertain response=${uncertain}`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'rotating',
      lifecycle: {
        pause: { allowed: false, reason: 'not_active' },
        revive: { allowed: false, reason: 'not_paused' },
        operation: {
          purpose: 'image_rotation',
          phase: 'conversation_reopen',
          started_at: localTime,
        },
      },
      runtime_update: {
        current_version: 'image-before',
        latest_version: 'image-after',
        target_version: 'image-after',
        update_available: true,
        state: 'failed',
        can_update: true,
        failure: { code: 'runtime_unavailable', message: 'The runtime could not be reopened.' },
        schedule: { timezone: 'Asia/Singapore', local_time: '02:00' },
        preserves_home: true,
      },
    };
    await boundary.route(page, baseURL!);
    const deletes: { path: string; key: string | undefined; body: string | null }[] = [];
    await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, async (route) => {
      const request = route.request();
      if (request.method() !== 'DELETE') return route.fallback();
      deletes.push({
        path: new URL(request.url()).pathname,
        key: request.headers()['idempotency-key'],
        body: request.postData(),
      });
      if (uncertain && deletes.length === 1)
        return route.fulfill({
          status: 503,
          json: { code: 'unavailable', detail: 'Delete response unavailable.' },
        });
      boundary.sandbox = {
        ...boundary.sandbox,
        status: 'retiring',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false } },
      };
      return route.fulfill({ status: 202, json: boundary.sandbox });
    });
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    await page.getByRole('button', { name: 'More actions', exact: true }).click();
    const remove = page.getByRole('menuitem', { name: /^Delete Sandbox/ });
    await expect(remove).not.toHaveAttribute('aria-disabled', 'true');
    await remove.click();
    const confirmation = page.getByRole('dialog', { name: 'Delete Sandbox', exact: true });
    await expect(confirmation).toContainText('stop the paused runtime update');
    expect(deletes).toEqual([]);
    await confirmation.getByRole('checkbox').check();
    await confirmation.getByRole('button', { name: 'Request deletion', exact: true }).click();
    if (uncertain) {
      await expect(page.getByRole('dialog', { name: 'Deleting Sandbox' })).toContainText(
        'not been confirmed',
      );
      await expect(
        page.getByRole('alert').filter({ hasText: 'Delete response unavailable' }),
      ).toBeVisible();
      expect(deletes).toHaveLength(1);
      await page.getByRole('button', { name: 'Check request', exact: true }).click();
    }
    await expect(page.getByText('Deleting 1 environment')).toBeVisible();
    expect(deletes).toHaveLength(uncertain ? 2 : 1);
    expect(deletes[0]!.key).toBeTruthy();
    if (uncertain) expect(deletes[1]).toEqual(deletes[0]);
    await page.reload();
    await page
      .getByRole('region', { name: 'Deletion request', exact: true })
      .getByRole('button', { name: 'View status', exact: true })
      .click();
    await page.getByRole('button', { name: /View Sandbox deletion status/ }).click();
    const progress = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
    await expect(progress).toBeVisible();
    await progress.getByText('Cleanup progress', { exact: true }).click();
    await expect(progress.getByRole('region', { name: 'Deletion progress' })).toBeVisible();
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted' })).toHaveCount(0);
    boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted' })).toBeVisible();
    expect(deletes).toHaveLength(uncertain ? 2 : 1);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
