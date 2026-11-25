import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const baseline = ['chrono-llm-public', 'ornn-api'];
const expanded = ['api-github', ...baseline];

test('LOCAL live Sandbox catalog recovery requires explicit Save and rereads the confirmed shared scope', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, reach: baseline };
  boundary.sandbox = { ...boundary.sandbox, status: 'active', nyxid_services: baseline };
  await boundary.route(page, baseURL!);
  let failCatalog = true;
  let catalogReads = 0;
  await page.route('**/api/v1/profile-authoring/services', (route) => {
    catalogReads++;
    return failCatalog
      ? route.fulfill({
          status: 503,
          json: {
            code: 'profile_services_unavailable',
            detail: '<html>private-catalog-diagnostic</html>',
          },
        })
      : route.fulfill({ json: [{ id: 'api-github', name: 'GitHub' }] });
  });
  const writes: { path: string; key?: string; body: unknown }[] = [];
  let status: 'pending' | 'succeeded' = 'pending';
  const receipt = () => ({
    id: 'access-local',
    agent_id: boundary.agent.id,
    status,
    services: expanded,
    reason: 'Owner revised scope.',
  });
  await page.route('**/api/agent-manager/v1/agents/agt_contract/service-access', (route) => {
    expect(route.request().method()).toBe('PUT');
    writes.push({
      path: new URL(route.request().url()).pathname,
      key: route.request().headers()['idempotency-key'],
      body: route.request().postDataJSON(),
    });
    return route.fulfill({ status: 202, json: receipt() });
  });
  await page.route('**/api/agent-manager/v1/agents/agt_contract/access-requests', (route) =>
    route.fulfill({ json: { items: writes.length ? [receipt()] : [] } }),
  );
  await page.goto('/sandboxes/sbx_route?tab=access');
  const access = page.getByRole('region', { name: 'Service access' });
  await expect(
    access.getByText('The catalog is temporarily unavailable. Try again.'),
  ).toBeVisible();
  await expect(access.getByRole('textbox', { name: 'Search services' })).toBeDisabled();
  await expect(page.locator('body')).not.toContainText('private-catalog-diagnostic');
  expect(writes).toEqual([]);
  failCatalog = false;
  await access.getByRole('button', { name: 'Try again', exact: true }).click();
  await expect(access.getByRole('checkbox', { name: 'GitHub' })).toBeEnabled();
  expect(catalogReads).toBeGreaterThanOrEqual(2);
  expect(writes).toEqual([]);
  await page.reload();
  await expect(access.getByRole('checkbox', { name: 'GitHub' })).toBeEnabled();
  expect(writes).toEqual([]);
  await access.getByRole('checkbox', { name: 'GitHub' }).check();
  await expect(
    access.getByText('These services are shared by all Agents in this Sandbox.'),
  ).toBeVisible();
  expect(writes).toEqual([]);
  await access.getByRole('button', { name: 'Save scope' }).click();
  await expect(access.getByText('Updating the key and notifying your Agent…')).toBeVisible();
  await expect(access.getByText('Agent key updated. Your Agent has been notified.')).toHaveCount(0);
  expect(writes).toHaveLength(1);
  expect(writes[0]?.body).toEqual({
    services: [...baseline, 'api-github'],
    expected_services: baseline,
  });
  expect(writes[0]?.key).toBeTruthy();
  status = 'succeeded';
  boundary.agent = { ...boundary.agent, reach: expanded };
  boundary.sandbox = { ...boundary.sandbox, nyxid_services: expanded };
  await expect(access.getByText('Agent key updated. Your Agent has been notified.')).toBeVisible();
  await page.reload();
  await expect(access.getByRole('checkbox', { name: 'GitHub' })).toBeChecked();
  for (const service of baseline) {
    await expect(access.getByRole('checkbox', { name: `${service} Required` })).toBeChecked();
    await expect(access.getByRole('checkbox', { name: `${service} Required` })).toBeDisabled();
  }
  await page.route('**/api/v1/agents/agt_contract/channel-usage', (route) =>
    route.fulfill({
      json: {
        agent_id: 'agt_contract',
        channel_usage: {
          active: false,
          has_history: false,
          first_used_at: null,
          last_used_at: null,
        },
      },
    }),
  );
  boundary.agent = {
    ...boundary.agent,
    id: 'agt_second',
    worktree: { ...boundary.agent.worktree, id: 'wkt_second' },
  };
  await page.goto('/agents/agt_second');
  await page.getByRole('button', { name: 'Show Agent details' }).click();
  const details = page.getByRole('complementary', { name: 'Agent details' });
  await expect(details.getByRole('list', { name: 'Services' })).toContainText('GitHub');
  await expect(details.getByRole('link', { name: 'Manage on Sandbox' })).toHaveAttribute(
    'href',
    '/sandboxes/sbx_route?tab=access',
  );
  expect(writes).toHaveLength(1);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
