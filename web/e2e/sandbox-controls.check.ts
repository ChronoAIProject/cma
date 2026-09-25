import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL shared Sandbox commands recover uncertain admission and clear on access loss', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  boundary.sandbox = { ...boundary.sandbox, owner: 'local-owner' };
  await boundary.route(page, baseURL!);
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: { owner: 'local-owner', ready: true, checked_at: localTime, services: [] },
    }),
  );
  let revoked = false;
  let revision = 7;
  let status = 'active';
  const posts: { key: string; body: { command: string; expected_revision: number } }[] = [];
  const sandbox = () => ({
    access: 'shared',
    id: 'sbx_shared',
    name: 'Research Sandbox',
    status,
    created_at: localTime,
    updated_at: localTime,
    last_activity_at: localTime,
    workspace_count: 0,
    worktree_count: 0,
    agent_count: 0,
  });
  const receipt = (terminal: boolean) => {
    const original = posts[posts.length > 2 ? 2 : 0]!.body;
    return {
      command_id: original.command === 'pause' ? 'scmd_pause' : 'scmd_revive',
      sandbox_id: 'sbx_shared',
      ...original,
      admitted_at: localTime,
      queue_deadline: new Date(Date.now() + 300_000).toISOString(),
      revision: terminal ? 2 : 1,
      state: terminal
        ? {
            status: 'applied',
            at: localTime,
            sandbox_revision: original.expected_revision + 1,
            disposition: 'scheduled',
          }
        : { status: 'queued' },
    };
  };
  await page.route('**/api/v1/**', async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const reply = (json: unknown, code = 200) => route.fulfill({ json, status: code });
    if (path === '/api/v1/organizations')
      return reply({ items: [{ id: 'org_research', display_name: 'Research', role: 'admin' }] });
    if (path === '/api/v1/sandboxes' && url.searchParams.has('organization'))
      return revoked
        ? reply({ code: 'resource_not_found', detail: 'Shared access removed.' }, 404)
        : reply({ items: [sandbox()] });
    if (!path.includes('sbx_shared') && !path.includes('/sandbox-commands/'))
      return route.fallback();
    if (revoked)
      return reply({ code: 'resource_not_found', detail: 'Shared access removed.' }, 404);
    if (path === '/api/v1/resource-access/sandbox/sbx_shared')
      return reply({
        owner: { kind: 'organization', organization_id: 'org_research' },
        capabilities: ['view_details', 'manage_sandbox'],
        policy_revision: 1,
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
      });
    if (path === '/api/v1/sandboxes/sbx_shared')
      return reply({ ...sandbox(), revision, workspaces: [] });
    if (path === '/api/v1/sandboxes/sbx_shared/commands') {
      expect(route.request().method()).toBe('POST');
      posts.push({
        key: route.request().headers()['idempotency-key']!,
        body: route.request().postDataJSON(),
      });
      if (posts.length === 1) {
        revision = 9;
        status = 'paused';
        return route.abort('connectionreset');
      }
      return reply(receipt(false), 202);
    }
    if (path.startsWith('/api/v1/sandbox-commands/')) return reply(receipt(posts.length === 2));
    throw new Error(`Unexpected shared Sandbox request ${path}`);
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('textbox', { name: 'Message' });
  await composer.fill('Keep this personal draft');
  const originalComposer = await composer.elementHandle();
  await page.getByRole('button', { name: 'Navigation scope', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Research', exact: true }).click();
  const navigation = page.locator('.scoped-navigation');
  await expect(navigation.locator('a[href="/sandboxes/sbx_shared"]')).toBeVisible();
  expect(await composer.evaluate((node, original) => node === original, originalComposer)).toBe(
    true,
  );
  await expect(composer).toHaveValue('Keep this personal draft');
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  await navigation.locator('a[href="/sandboxes/sbx_shared"]').click();
  const content = page.locator('.console__surface .sandbox-page');
  await expect(content.getByRole('button', { name: 'Pause Sandbox' })).toBeVisible();
  await expect(content.getByRole('button', { name: 'Retire Sandbox' })).toHaveCount(0);
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
            .evaluate((node) => node.getBoundingClientRect().right <= innerWidth + 1),
        )
        .toBe(true);
      await expect
        .poll(() =>
          content
            .getByRole('button', { name: 'Pause Sandbox' })
            .evaluate(
              (node) =>
                node.getBoundingClientRect().right <= innerWidth &&
                node.getBoundingClientRect().left >= 0,
            ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`sandbox-controls-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  await content.getByRole('button', { name: 'Pause Sandbox' }).click();
  await expect(
    content.getByText('The result is not confirmed. Retry to recover this request.'),
  ).toBeVisible();
  await content.getByRole('button', { name: 'Refresh Sandbox access' }).click();
  await content.getByRole('button', { name: 'Retry Sandbox request' }).click();
  await expect(
    content.getByText('Request applied. Sandbox status shows runtime progress.'),
  ).toBeVisible();
  expect(posts).toHaveLength(2);
  expect(posts[1]).toEqual(posts[0]);
  expect(posts[0]?.body).toEqual({ command: 'pause', expected_revision: 7 });
  await content.getByRole('button', { name: 'Done' }).click();
  await content.getByRole('button', { name: 'Resume Sandbox' }).click();
  await expect(content.getByText('Request queued. Checking progress…')).toBeVisible();
  expect(posts[2]?.body).toEqual({ command: 'revive', expected_revision: 9 });
  expect(posts[2]?.key).not.toBe(posts[0]?.key);
  revoked = true;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByRole('heading', { name: 'Sandbox unavailable' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Retry Sandbox request' })).toHaveCount(0);
  await expect(page.getByText('Research Sandbox', { exact: true })).toHaveCount(0);
  expect(posts).toHaveLength(3);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
