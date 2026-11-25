import { expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { channelCapabilities, cmaSession, gatewaySession } from '../build/fixtures/eventGateway';

/** `identity` holds the CMA sign-in answer until it settles. */
export async function setup(
  page: Page,
  baseURL: string,
  identity?: Promise<unknown>,
  managedSetup = true,
  serviceAuthorization = false,
) {
  const boundary = new LocalStartup();
  const writes: { body: string | null; key: string | undefined; url: string }[] = [];
  await boundary.route(page, baseURL);
  await page.route('**/auth/session', async (route) => {
    await identity;
    return route.fulfill({ json: { ...cmaSession, service_authorization: serviceAuthorization } });
  });
  await page.route('**/api/v1/bots?**', (route) =>
    route.fulfill({ json: { items: [], next_after: null } }),
  );
  await page.route('**/api/event-gateway/**', (route) => {
    const request = route.request(),
      path = new URL(request.url()).pathname;
    if (request.method() !== 'GET') {
      writes.push({
        body: request.postData(),
        key: request.headers()['idempotency-key'],
        url: path,
      });
      return route.fulfill({ status: 503, json: { detail: 'Local result unconfirmed' } });
    }
    if (path.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
    if (path.endsWith('/v1/channel-capabilities'))
      return route.fulfill({
        json: {
          ...channelCapabilities,
          platforms: managedSetup
            ? [
                {
                  id: 'telegram',
                  label: 'Telegram',
                  connection_methods: ['existing_credential', 'managed_create'],
                  optional_capabilities: [],
                },
              ]
            : [],
          relay_authoring: {
            ...channelCapabilities.relay_authoring,
            platforms: channelCapabilities.relay_authoring.platforms.map((platform) => ({
              ...platform,
              verification_modes: [
                ...platform.verification_modes,
                { id: 'native_event', writes_enabled: true },
              ],
            })),
          },
        },
      });
    if (path.endsWith('/v1/channel-connections'))
      return route.fulfill({
        json: {
          items: [
            { connection_ref: 'owned-ready', status: 'ready', label: 'Owned Telegram bot' },
            {
              connection_ref: 'owned-pending',
              status: 'awaiting_platform',
              label: 'Pending Telegram bot',
            },
          ],
        },
      });
    if (path.endsWith('/v1/channels') || path.endsWith('/v1/connections'))
      return route.fulfill({ json: { items: [], next_cursor: null } });
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.goto('/event-gateway');
  await page
    .getByRole('region', { name: 'Channel workspace', exact: true })
    .getByRole('button', { name: 'New channel', exact: true })
    .click();
  await expect(page.getByRole('button', { name: 'Create channel', exact: true })).toBeVisible();
  return writes;
}
