import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL creation labels, tree filtering and details preserve original facts across later Trigger use', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let allowed = true;
  await page.route('**/api/v1/agents/agt_contract/provenance', (route) =>
    route.fulfill({
      status: allowed ? 200 : 404,
      json: allowed
        ? {
            agent_id: boundary.agent.id,
            created_at: boundary.agent.created_at,
            creation_origin: 'manual',
            details: {
              created_by: { kind: 'human', subject: 'private-creator-reference' },
              trigger: null,
              channel_bridge: null,
              workspace_profile_index: null,
            },
          }
        : { status: 404, code: 'not_found', detail: 'Unavailable' },
    }),
  );
  await page.route('**/api/v1/agents/agt_contract/source', (route) =>
    route.fulfill({
      json: {
        agent_id: boundary.agent.id,
        origin: { type: 'workspace_profile' },
        trigger: {
          title: 'Project introduction',
          public_id: 'pub_shortcut',
          revision: 2,
          quick_messages: [],
        },
      },
    }),
  );
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await expect(page.getByRole('menu', { name: 'More Agent actions' })).toContainText(
    'Created manually',
  );
  await expect(
    page.getByRole('menuitem', { name: 'Published Trigger: Project introduction' }),
  ).toHaveAttribute('href', '/t/pub_shortcut');
  await page.keyboard.press('Escape');
  await expect(page.getByTitle('Created manually')).toBeVisible();
  const filter = page.getByRole('combobox', { name: 'Agent creation' });
  await filter.selectOption('trigger');
  await expect(page.getByText('No matching Agents in this tree')).toBeVisible();
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await expect(page.getByRole('menu', { name: 'More Agent actions' })).toContainText(
    'Created manually',
  );
  await page.keyboard.press('Escape');
  await filter.selectOption('manual');
  await expect(page.getByTitle('Created manually')).toBeVisible();
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
  const details = page.getByRole('complementary', { name: 'Agent details' });
  await expect(details.getByText('Created manually')).toBeVisible();
  await expect(page.getByText('private-creator-reference')).toHaveCount(0);
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
      .toBe(true);
    await page.screenshot({
      path: info.outputPath(`creation-${String(width)}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  allowed = false;
  await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await expect(page.getByRole('menu', { name: 'More Agent actions' })).toContainText(
    'Creation origin unavailable',
  );
  await page.keyboard.press('Escape');
  await expect(details.getByText('Creation origin unavailable')).toBeVisible();
  await expect(page.getByTitle('Created manually')).toHaveCount(0);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
