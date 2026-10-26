import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL channel history retains manual origin and drafts while current authority controls private binding details', async ({
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
  let detached = false;
  let manager = true;
  const binding = {
    binding_id: 'binding-review',
    bridge_id: 'bridge-review',
    partition_id: 'private-partition',
    generation: 1,
    previous_binding_id: null,
    attached_at: localTime,
    first_used_at: null,
    last_used_at: null,
  };
  const used = { ...binding, first_used_at: localTime, last_used_at: localTime };
  const event = (revision: number) => ({
    revision,
    actor: { kind: 'human', subject: 'local-manager' },
    at: localTime,
    detached_at: revision === 3 ? localTime : null,
    reason: revision === 1 ? 'attachment' : revision === 2 ? 'accepted_command' : 'detachment',
    change: {
      kind: revision === 1 ? 'attach' : revision === 2 ? 'use' : 'detach',
      binding_id: binding.binding_id,
    },
    before: revision === 1 ? [] : [revision === 2 ? binding : used],
    after: revision === 3 ? [] : [revision === 1 ? binding : used],
  });
  await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
    route.fulfill({
      json: {
        capabilities: ['view_details', 'view_history', ...(manager ? ['manage_bridge'] : [])],
        revalidate_at: null,
        policy_revision: manager ? 1 : 2,
      },
    }),
  );
  await page.route('**/api/v1/agents/agt_contract/provenance', (route) =>
    route.fulfill({
      json: {
        agent_id: 'agt_contract',
        creation_origin: 'manual',
        created_at: localTime,
      },
    }),
  );
  await page.route('**/api/v1/agents/agt_contract/channel-usage**', (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/history')) {
      if (!manager)
        return route.fulfill({
          status: 403,
          json: { code: 'resource_forbidden', detail: 'Management access removed' },
        });
      const revision = Number(url.searchParams.get('cursor') ?? 0) + 1;
      return route.fulfill({
        json: {
          agent_id: 'agt_contract',
          items: [event(revision)],
          ...(revision < 3 ? { next: String(revision) } : {}),
        },
      });
    }
    return route.fulfill({
      json: {
        agent_id: 'agt_contract',
        channel_usage: {
          active: !detached,
          has_history: true,
          first_used_at: detached ? localTime : null,
          last_used_at: detached ? localTime : null,
        },
        ...(manager ? { bindings: detached ? [] : [binding] } : {}),
      },
    });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('textbox', { name: 'Message' });
  await composer.fill('Keep this draft while checking channel history');
  const node = await composer.elementHandle();
  const tree = page.locator('.resource-tree');
  await expect(tree.getByText('Channel linked', { exact: true })).toBeVisible();
  await tree.getByLabel('Agent channel activity', { exact: true }).selectOption('active');
  await expect(tree.locator('a[href="/agents/agt_contract"]')).toBeVisible();
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
  await expect(page.getByText('Linked to a channel', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Channel history', exact: true }).click();
  const modal = page.getByRole('dialog', { name: 'Channel history' });
  await expect(modal.getByRole('heading', { name: 'Channel attached' })).toBeVisible();
  await modal.getByText('Binding changes for event 1').click();
  await expect(modal.getByText('binding-review', { exact: true })).toBeVisible();
  await expect(modal.getByText('None recorded')).toHaveCount(2);
  await expect(modal.getByText('private-partition')).toHaveCount(0);
  await modal.getByRole('button', { name: 'Next channel events' }).click();
  await expect(modal.getByRole('heading', { name: 'Channel message accepted' })).toBeVisible();
  await modal.getByRole('button', { name: 'Next channel events' }).click();
  await expect(modal.getByRole('heading', { name: 'Channel detached' })).toBeVisible();
  const modalNode = await modal.elementHandle();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(modal.getByRole('heading', { name: 'Channel detached' })).toBeVisible();
  expect(await modal.evaluate((element, previous) => element === previous, modalNode)).toBe(true);
  await modal.getByText('Binding changes for event 3').click();
  await expect(modal.getByRole('button', { name: 'Next channel events' })).toBeDisabled();
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          modal.evaluate((element) =>
            [...element.querySelectorAll('button,dd')].every((item) => {
              const rect = item.getBoundingClientRect();
              return rect.left >= 0 && rect.right <= innerWidth + 1;
            }),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`channel-history-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  manager = false;
  await modal.getByRole('button', { name: 'Refresh channel history' }).click();
  await expect(modal.getByText('Current access does not allow channel history.')).toBeVisible();
  await expect(modal.getByText('binding-review', { exact: true })).toHaveCount(0);
  await modal.getByRole('button', { name: 'Close', exact: true }).click();
  detached = true;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await page.setViewportSize({ width: 1440, height: 1000 });
  await expect(page.getByText(/Previously linked to a channel/)).toBeVisible();
  await tree.getByLabel('Agent channel activity', { exact: true }).selectOption('detached');
  await expect(tree.getByText('Channel history', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Channel history', exact: true })).toHaveCount(0);
  await expect(page.getByText('Created manually', { exact: true }).first()).toBeVisible();
  expect(await composer.evaluate((element, previous) => element === previous, node)).toBe(true);
  await expect(composer).toHaveValue('Keep this draft while checking channel history');
  expect(boundary.unexpected).toEqual([]);
});
