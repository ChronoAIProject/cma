import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL organization inventory, retained creation export and shared Agent controls follow current access', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let admin = false;
  let revoked = false;
  const forbidden: string[] = [];
  const writes: unknown[] = [];
  const agent = {
    access: 'shared',
    id: 'agt_shared',
    title: 'Project review',
    status: 'ready',
    created_at: localTime,
    updated_at: localTime,
  };
  const row = {
    agent_id: agent.id,
    title: agent.title,
    status: agent.status,
    created_at: localTime,
    updated_at: localTime,
    creation_origin: 'manual',
  };
  await page.route('**/api/v1/**', async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const reply = (json: unknown, status = 200) => route.fulfill({ status, json });
    if (path === '/api/v1/sandboxes') return reply({ items: [] });
    if (path === '/api/v1/organizations')
      return reply({
        items: [{ id: 'org', role: admin ? 'admin' : 'viewer', display_name: 'Research' }],
      });
    if (revoked)
      return reply(
        { code: 'resource_not_found', detail: 'Resource access is no longer available.' },
        404,
      );
    if (path === '/api/v1/agents/provenance') {
      if (url.searchParams.has('creation_origin') && !url.searchParams.has('cursor'))
        return reply({ items: [], next: 'agt_previous' });
      return reply({
        items: [
          row,
          {
            ...row,
            agent_id: 'agt_shared_retained',
            title: 'Retained investigation',
            status: 'deleted',
          },
        ],
      });
    }
    if (
      [
        '/api/v1/resource-access/agent/agt_shared',
        '/api/v1/resource-access/agent/agt_shared_retained',
      ].includes(path)
    )
      return reply({
        capabilities: [
          'view_details',
          'view_history',
          ...(admin ? ['interact', 'resolve_approval'] : []),
        ],
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
        policy_revision: 1,
      });
    if (path === '/api/v1/agents/agt_shared') return reply(agent);
    if (path === '/api/v1/agents/agt_shared/channel-usage')
      return reply({
        agent_id: agent.id,
        channel_usage: {
          active: false,
          has_history: false,
          first_used_at: null,
          last_used_at: null,
        },
      });
    if (path === '/api/v1/agents/agt_shared/provenance')
      return reply({ agent_id: agent.id, created_at: localTime, creation_origin: 'manual' });
    if (path === '/api/v1/agents/agt_shared/transcript')
      return reply({ agent_id: agent.id, through_seq: 0, turns: [] });
    if (path === '/api/v1/agents/agt_shared/events') return reply({ items: [] });
    if (path === '/api/v1/agui/events')
      return route.fulfill({ contentType: 'text/event-stream', body: ': keepalive\n\n' });
    if (
      path === '/api/v1/agents/agt_shared/input' &&
      route.request().method() === 'POST' &&
      admin
    ) {
      expect(route.request().headers()['idempotency-key']).toBeTruthy();
      writes.push(route.request().postDataJSON());
      return reply({ queued: false }, 202);
    }
    forbidden.push(path);
    return reply({ code: 'fixture_forbidden', detail: 'No grant for this resource.' }, 403);
  });
  await page.goto('/organizations/org/agents');
  await expect(page.getByRole('link', { name: 'Project review' })).toBeVisible();
  await expect(
    page.locator('.agent-inventory').getByText('Research · Organization viewer'),
  ).toBeVisible();
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          page
            .locator('.frame__center')
            .evaluate((element) => element.getBoundingClientRect().right <= innerWidth + 1),
        )
        .toBe(true);
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      await expect
        .poll(() =>
          page
            .locator('.agent-inventory')
            .evaluate((element) =>
              [...element.querySelectorAll('button, a, time')].every(
                (item) => item.getBoundingClientRect().right <= innerWidth + 1,
              ),
            ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`inventory-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  await page.getByRole('button', { name: 'Creation origin' }).click();
  await page.getByRole('menuitemradio', { name: 'Created manually', exact: true }).click();
  await expect(
    page.getByText('No matches on this page. Continue to check the next page.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Next page', exact: true }).click();
  await expect(page.getByRole('link', { name: 'Project review' })).toBeVisible();
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export this page' }).click();
  expect((await download).suggestedFilename()).toBe('cma-agents-page.json');
  await page.getByRole('link', { name: 'Project review' }).click();
  await expect(
    page.getByText('You can view this Agent. Your current access does not allow interaction.'),
  ).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Message' })).toHaveCount(0);
  await expect(page.getByText('Created manually')).toBeVisible();
  await page.screenshot({
    path: info.outputPath('shared-viewer-320.png'),
    fullPage: true,
    animations: 'disabled',
  });
  admin = true;
  await page.getByRole('button', { name: 'Show Agent details' }).click();
  await page.getByRole('button', { name: 'Refresh access' }).click();
  await page.getByRole('button', { name: 'Close Agent details' }).click();
  await page
    .getByRole('textbox', { name: 'Message', exact: true })
    .fill('Review from the current organization admin.');
  await page.getByRole('button', { name: 'Send message' }).click();
  await expect.poll(() => writes.length).toBe(1);
  expect(writes).toEqual([{ text: 'Review from the current organization admin.' }]);
  await page.getByRole('button', { name: 'Show Agent details' }).click();
  revoked = true;
  await page.getByRole('button', { name: 'Refresh access' }).click();
  await expect(page.getByRole('heading', { name: 'Agent unavailable' })).toBeVisible();
  await expect(page.getByText('Project review')).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Message' })).toHaveCount(0);
  expect(forbidden).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
