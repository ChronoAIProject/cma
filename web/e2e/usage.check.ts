import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL legacy operator allocation usage retains filters and mobile layout', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'local-owner',
        display_name: 'Local owner',
        capabilities: ['use_own_sandboxes', 'create_sandbox', 'read_all_sandboxes'],
      },
    }),
  );
  const totals = {
    uptime_seconds: 3661,
    cpu_core_hours: 1.017,
    memory_gib_hours: 2.034,
    disk_gib_hours: 20.34,
    cpu_millis_seconds: 3661000,
    memory_mib_seconds: 7497728,
    disk_gib_seconds: 73224,
  };
  const allocation = { running: true, cpu_millis: 1000, memory_mib: 2048, disk_gib: 20 };
  const summary = {
    as_of: 1793577600,
    measurement: 'allocated_resource_time',
    sandbox_count: 1,
    running_count: 1,
    incomplete_lifecycle_count: 1,
    tracking_started_at: 1793573939,
    totals,
  };
  const row = {
    sandbox_id: 'sbx-research',
    owner: 'local-owner',
    name: 'Research',
    status: 'active',
    allocation,
    created_at: 1793570000,
    tracking_started_at: 1793573939,
    as_of: summary.as_of,
    complete_lifecycle: false,
    totals,
  };
  await page.route('**/api/v1/admin/sandboxes**', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            ...boundary.sandbox,
            owner_name: 'Local owner',
            agent_count: 1,
            last_activity_at: '2026-09-05T00:00:00Z',
            runtime_version: null,
            retired_at: null,
          },
        ],
        next: null,
      },
    }),
  );
  const requests: string[] = [];
  await page.route('**/api/v1/**usage**', (route) => {
    const url = new URL(route.request().url());
    requests.push(url.pathname + url.search);
    return route.fulfill({ json: summary });
  });
  await page.route('**/api/v1/**usage/sandboxes**', (route) => {
    const url = new URL(route.request().url());
    requests.push(url.pathname + url.search);
    return route.fulfill({
      json: url.pathname.endsWith('/events')
        ? {
            items: [
              {
                transaction_id: 'cma-usage:sbx-research:2',
                revision: 2,
                source_seq: 3,
                kind: 'resumed',
                started_at: 1793573939,
                ended_at: 1793577600,
                status: 'active',
                allocation,
                next_allocation: allocation,
                quantity: totals,
              },
            ],
            next: null,
          }
        : { items: [row], next: null, as_of: summary.as_of },
    });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/operations');
  await page.getByRole('tab', { name: 'Usage', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Usage user' })).toBeVisible();
  await page.getByRole('button', { name: 'Usage user' }).click();
  await page.getByRole('menuitemradio', { name: 'Local owner', exact: true }).click();
  await page.getByRole('button', { name: 'Usage sandbox status' }).click();
  await page.getByRole('menuitemradio', { name: 'active', exact: true }).click();
  await page.getByRole('searchbox', { name: 'Sandbox ID' }).fill('research');
  await page.getByRole('button', { name: 'Apply filters' }).click();
  await expect(page.getByRole('button', { name: 'Research', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Sort by CPU (core-hours) ascending' }).click();
  await expect(page.getByRole('columnheader', { name: /CPU/ })).toHaveAttribute(
    'aria-sort',
    'ascending',
  );
  await page.getByRole('button', { name: 'Sort by CPU (core-hours) descending' }).click();
  await expect(page.getByRole('columnheader', { name: /CPU/ })).toHaveAttribute(
    'aria-sort',
    'descending',
  );
  expect(requests.some((path) => path.includes('/admin/usage/sandboxes?limit=200'))).toBe(true);
  await page.screenshot({
    path: info.outputPath('operations-usage-desktop.png'),
    fullPage: true,
    animations: 'disabled',
  });
  await page.setViewportSize({ width: 320, height: 1000 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  );
  await page.getByRole('button', { name: 'Usage sandbox status' }).click();
  await page.getByRole('menuitemradio', { name: 'All statuses' }).click();
  await page.getByRole('searchbox', { name: 'Sandbox ID' }).fill('does-not-exist');
  await page.getByRole('button', { name: 'Apply filters' }).click();
  await expect(page.getByText('No sandbox usage matches these filters.')).toBeVisible();
  await page.screenshot({
    path: info.outputPath('operations-usage-mobile.png'),
    fullPage: true,
    animations: 'disabled',
  });
  expect(boundary.errors).toEqual([]);
});
