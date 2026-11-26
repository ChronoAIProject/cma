import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL Sandbox scope additions enable Save after a ready connection check', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, reach: ['chrono-llm-public', 'ornn-api'] };
  await boundary.route(page, baseURL!);
  const checks: unknown[] = [];
  const writes: unknown[] = [];
  let connected = false;
  await page.route('**/auth/connections/check', (route) => {
    const body = route.request().postDataJSON();
    checks.push(body);
    const needsGitHub =
      body.target.type === 'agent_service_access' && body.target.services.includes('api-github');
    return route.fulfill({
      json: {
        owner: 'local-owner',
        ready: !needsGitHub || connected,
        checked_at: localTime,
        services: needsGitHub
          ? [
              {
                service: 'api-github',
                display_name: 'GitHub',
                kind: 'nyxid',
                state: connected ? 'satisfied' : 'missing',
                connection_mode: 'hosted',
              },
            ]
          : [],
      },
    });
  });
  await page.route('**/api/agent-manager/v1/agents/*/service-access', (route) => {
    writes.push(route.request().postDataJSON());
    return route.fulfill({
      json: {
        id: 'scope-update',
        agent_id: boundary.agent.id,
        status: 'succeeded',
        services: route.request().postDataJSON().services,
        reason: 'Owner revised scope.',
      },
    });
  });
  await page.goto('/sandboxes/sbx_route');
  await page.getByRole('tab', { name: 'Access' }).click();
  await expect(page.getByRole('button', { name: 'Save scope' })).toBeDisabled();
  await page.getByRole('checkbox', { name: 'GitHub', exact: true }).check();
  await expect(page.getByText('Verify connections for GitHub before saving.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save scope' })).toBeDisabled();
  expect(writes).toEqual([]);
  connected = true;
  await page.getByRole('button', { name: 'Check connections again' }).click();
  await expect(page.getByRole('button', { name: 'Save scope' })).toBeEnabled();
  await page.getByRole('button', { name: 'Save scope' }).click();
  await expect(page.getByText('Agent key updated. Your Agent has been notified.')).toBeVisible();
  expect(writes).toHaveLength(1);
  expect(checks).toContainEqual({
    target: {
      type: 'agent_service_access',
      id: boundary.agent.id,
      services: expect.arrayContaining(['api-github']),
    },
  });
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
  await page.getByRole('checkbox', { name: 'GitHub', exact: true }).uncheck();
  await expect(page.getByRole('button', { name: 'Save scope' })).toBeEnabled();
  await page.getByRole('button', { name: 'Save scope' }).click();
  await expect(page.getByText('Agent key updated. Your Agent has been notified.')).toBeVisible();
  expect(writes).toHaveLength(2);
});

for (const condition of ['missing_connection', 'unavailable', 'different_owner'] as const) {
  test(`LOCAL Sandbox scope save remains blocked for ${condition}`, async ({ page, baseURL }) => {
    const boundary = new LocalStartup();
    boundary.agent = { ...boundary.agent, reach: ['chrono-llm-public', 'ornn-api'] };
    await boundary.route(page, baseURL!);
    await page.route('**/auth/connections/check', (route) => {
      const scope = route.request().postDataJSON().target.type === 'agent_service_access';
      return scope && condition === 'unavailable'
        ? route.fulfill({ status: 503, json: { code: 'unavailable' } })
        : route.fulfill({
            json: {
              owner: scope && condition === 'different_owner' ? 'another-owner' : 'local-owner',
              ready: !scope || condition !== 'missing_connection',
              checked_at: localTime,
              services: [],
            },
          });
    });
    await page.goto('/sandboxes/sbx_route');
    await page.getByRole('tab', { name: 'Access' }).click();
    await page.getByRole('checkbox', { name: 'GitHub', exact: true }).check();
    if (condition === 'missing_connection') {
      await expect(
        page.getByRole('button', { name: 'Connect services before saving' }),
      ).toBeVisible();
    } else {
      await expect(page.getByRole('button', { name: 'Retry connection check' })).toBeVisible();
    }
    await expect(page.getByRole('button', { name: 'Save scope' })).toBeDisabled();
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
