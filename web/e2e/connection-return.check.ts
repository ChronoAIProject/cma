import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL Connect all reuses the returned popup and resumes the pending message once', async ({
  page,
  context,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
    route.fulfill({
      json: {
        capabilities: ['view_details', 'view_history', 'interact'],
        revalidate_at: null,
        policy_revision: 1,
      },
    }),
  );
  const services = ['api-github', 'ornn-api'];
  const connected = new Set<string>();
  const starts: string[] = [];
  const messages: unknown[] = [];
  let releaseVerification = false;
  let requireConnections = false;
  const popups: unknown[] = [];
  page.on('popup', (popup) => popups.push(popup));

  // Isolated identity fixture: these requests never reach NyxID or another server.
  await context.route('https://identity.example.test/**', async (route) => {
    const url = new URL(route.request().url());
    const service = url.pathname.split('/').at(-1)!;
    expect(services).toContain(service);
    if (url.pathname.startsWith('/complete/')) {
      connected.add(service);
      return route.fulfill({
        status: 302,
        headers: { location: `${baseURL}/connection-return?status=completed` },
      });
    }
    return route.fulfill({
      contentType: 'text/html',
      body: `<!doctype html><title>Local sign-in</title><a href="/complete/${service}">Complete connection</a>`,
    });
  });
  await context.route('**/auth/session', (route) =>
    route.fulfill({ json: { authenticated: false } }),
  );
  await page.route('**/auth/connections/**', async (route) => {
    const action = new URL(route.request().url()).pathname.split('/').at(-1);
    const body = route.request().postDataJSON();
    if (action === 'start') {
      starts.push(body.service_id);
      return route.fulfill({
        json: {
          owner: 'local-owner',
          service_id: body.service_id,
          intent_id: body.intent_id,
          hosted_url: `https://identity.example.test/connect/${body.service_id}`,
          expires_at: new Date(Date.now() + 600000).toISOString(),
        },
      });
    }
    expect(action).toBe('check');
    return route.fulfill({
      json: {
        owner: 'local-owner',
        ready: !requireConnections || (releaseVerification && connected.size === services.length),
        checked_at: localTime,
        services: services.map((service) => ({
          service,
          display_name: service,
          kind: 'nyxid',
          state:
            !requireConnections || (releaseVerification && connected.has(service))
              ? 'satisfied'
              : 'missing',
          connection_mode: 'hosted',
        })),
      },
    });
  });
  await page.route('**/api/v1/agui/run', async (route) => {
    const request = route.request().postDataJSON();
    messages.push(request);
    return route.fulfill({
      contentType: 'text/event-stream',
      body: ['RUN_STARTED', 'RUN_FINISHED']
        .map(
          (type) =>
            `data: ${JSON.stringify({ type, threadId: request.threadId, runId: request.runId })}\n\n`,
        )
        .join(''),
    });
  });

  await page.goto('/agents/agt_contract');
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Keep my pending message');
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  requireConnections = true;
  await page.getByRole('button', { name: 'Send message', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Connect your accounts' });
  const popupPromise = page.waitForEvent('popup');
  await dialog.getByRole('button', { name: 'Connect all', exact: true }).click();
  const popup = await popupPromise;
  await popup.getByRole('link', { name: 'Complete connection' }).click();
  await expect(popup.getByText('Returning to CMA', { exact: true })).toBeVisible();
  // Delay the authoritative result beyond the old 50 ms automatic-close timer.
  await popup.waitForTimeout(250);
  expect(popup.isClosed()).toBe(false);
  expect(messages).toEqual([]);
  releaseVerification = true;
  await expect(popup).toHaveURL('https://identity.example.test/connect/ornn-api');
  await popup.getByRole('link', { name: 'Complete connection' }).click();
  await expect.poll(() => popup.isClosed()).toBe(true);
  await expect(dialog).toHaveCount(0);
  await expect.poll(() => messages.length).toBe(1);
  expect(starts).toEqual(services);
  expect(popups).toHaveLength(1);
  expect(messages[0]).toMatchObject({
    messages: [{ role: 'user', content: 'Keep my pending message' }],
  });
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
