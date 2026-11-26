import { expect, test, type Page, type TestInfo } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { navigation, resourceRow } from './resource-lifecycle.fixture';

async function captureThemes(page: Page, info: TestInfo, name: string) {
  for (const dark of [true, false]) {
    await page.evaluate(
      (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
      dark,
    );
    await page.screenshot({
      path: info.outputPath(`${name}-${dark ? 'dark' : 'light'}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
}

for (const purpose of ['image_rotation', 'revive'] as const) {
  test(`LOCAL task menu retires its parked ${purpose} environment through explicit confirmation`, async ({
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
          operation_id: 'parked-task-update',
          started_at: localTime,
          actions: ['observe', 'retire'],
        },
      },
    };
    const sibling = {
      ...boundary.workspace.agents[0]!,
      id: 'agt_other_task',
      worktree_id: 'wkt_other_task',
      title: 'Another task in this environment',
    };
    boundary.workspace = {
      ...boundary.workspace,
      agents: [...boundary.workspace.agents, sibling],
      agent_count: 2,
    };
    await boundary.route(page, baseURL!);
    await page.route(`**/api/v1/agents/${sibling.id}/channel-usage`, (route) =>
      route.fulfill({
        json: {
          agent_id: sibling.id,
          channel_usage: {
            active: false,
            has_history: false,
            first_used_at: null,
            last_used_at: null,
          },
        },
      }),
    );
    await page.route(`**/api/v1/agents/${sibling.id}/provenance`, (route) =>
      route.fulfill({
        json: { agent_id: sibling.id, created_at: localTime, creation_origin: 'legacy_unknown' },
      }),
    );
    const deletes: { path: string; key?: string; body: string | null }[] = [];
    let inaccessible = false;
    const uncertain = purpose === 'revive';
    await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, async (route) => {
      const request = route.request();
      if (request.method() === 'GET')
        return inaccessible
          ? route.fulfill({ status: 404, json: { code: 'not_found', detail: 'Not accessible.' } })
          : route.fallback();
      expect(request.method()).toBe('DELETE');
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
        revision: boundary.sandbox.revision + 1,
        status: 'retiring',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false } },
      };
      return route.fulfill({ status: 202, json: boundary.sandbox });
    });
    await page.goto('/agent-manager');
    await navigation(page, true);
    await page.locator('.runtime-attention > summary').click();
    const menu = resourceRow(page, 'agent', boundary.agent.id).getByRole('button', {
      name: /^Options for/,
    });
    await menu.click();
    const remove = page.getByRole('menuitem', { name: /^Delete environment…/ });
    await expect(remove).not.toHaveAttribute('aria-disabled', 'true');
    await expect(page.getByRole('menuitem', { name: /^Stop\b/ })).toContainText(
      'Delete environment',
    );
    await expect(page.getByRole('menuitem', { name: /^Delete (?!environment)/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await expect(page.getByRole('menuitem', { name: /^Retry runtime update/ })).toBeVisible();
    await captureThemes(page, info, 'task-menu');
    await remove.click();
    const confirmation = page.getByRole('dialog', { name: 'Delete Sandbox', exact: true });
    await expect(confirmation).toContainText(boundary.sandbox.name);
    await expect(confirmation).toContainText('whole Sandbox environment');
    await expect(confirmation).toContainText(sibling.title);
    await expect(confirmation).toContainText('This is more than stopping one agent');
    await captureThemes(page, info, 'confirmation');
    expect(deletes).toEqual([]);
    expect(boundary.writes).toEqual([]);
    await confirmation.getByRole('button', { name: 'Keep environment', exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    expect(deletes).toEqual([]);
    await menu.click();
    await remove.click();
    await confirmation.getByRole('checkbox').check();
    await confirmation.getByRole('button', { name: 'Request deletion', exact: true }).dblclick();
    if (uncertain) {
      const pending = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
      await expect(pending).toContainText('not been confirmed');
      expect(deletes).toHaveLength(1);
      await pending.getByRole('button', { name: 'Check request', exact: true }).click();
    }
    await expect(page.getByText('Deleting 1 environment')).toBeVisible();
    expect(deletes).toHaveLength(uncertain ? 2 : 1);
    expect(deletes[0]).toMatchObject({
      path: `/api/v1/sandboxes/${boundary.sandbox.id}`,
      key: expect.any(String),
      body: null,
    });
    expect(deletes[0]!.key).toBeTruthy();
    if (uncertain) expect(deletes[1]).toEqual(deletes[0]);
    await page.reload();
    await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
    await page.getByRole('button', { name: /View Sandbox deletion status/ }).click();
    const progress = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
    await expect(progress).toBeVisible();
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted', exact: true })).toHaveCount(0);
    await captureThemes(page, info, 'accepted-progress');
    inaccessible = true;
    await expect(progress).toContainText('Deletion could not be verified');
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted', exact: true })).toHaveCount(0);
    inaccessible = false;
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'retired',
      revision: boundary.sandbox.revision + 1,
    };
    await progress.getByRole('button', { name: 'Refresh status', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted', exact: true })).toBeVisible();
    expect(deletes).toHaveLength(uncertain ? 2 : 1);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
