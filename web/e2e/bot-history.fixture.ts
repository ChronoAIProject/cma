import type { Page } from '@playwright/test';

// Synthetic public responses; the browser still runs the real creation wizard and cleanup.
export async function routeHistoryBot(page: Page) {
  const bot = {
    bot_id: 'bot_history',
    owner: 'local-owner',
    name: 'History consumer',
    nyxid_bot_id: 'external-fixture',
    profile: { id: 'agp_fixture', revision: 1 },
    provider: 'cma',
    state: 'active',
    failure: null,
    operation: null,
    channel: null,
    credentials_expire_at: null,
    created_at: '2026-09-23T00:00:00Z',
    updated_at: '2026-09-23T00:00:00Z',
    last_test: null,
  };
  const writes: string[] = [];
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'local-owner',
        display_name: 'Local owner',
        capabilities: ['manage_own_bots', 'use_own_sandboxes'],
      },
    }),
  );
  await page.route('**/api/v1/bots**', (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() === 'POST' && path === '/api/v1/bots') {
      writes.push(path);
      return route.fulfill({ status: 201, json: bot });
    }
    if (request.method() !== 'GET') throw new Error('Unexpected Bot mutation');
    if (path === '/api/v1/bots/connections')
      return route.fulfill({
        json: [{ connection_ref: 'connection-fixture', label: 'Research channel' }],
      });
    if (path === '/api/v1/bots')
      return route.fulfill({
        json: {
          items: writes.length ? [bot] : [],
          next_after: null,
        },
      });
    if (path === '/api/v1/bots/bot_history') return route.fulfill({ json: bot });
    throw new Error(`Unexpected Bot read: ${path}`);
  });
  return writes;
}
