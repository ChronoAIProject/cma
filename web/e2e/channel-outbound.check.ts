import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';

test('LOCAL producer explains provider failure and retains its receipt through separate-message recovery', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) => route.fulfill({ json: cmaSession }));
  await page.route('**/api/v1/bots?**', (route) =>
    route.fulfill({ json: { items: [], next_after: null } }),
  );
  const calls: { method: string; path: string; key: string | undefined; body: unknown }[] = [];
  const base = '/api/event-gateway/v1/channels/channel-local/outbound-messages';
  const receipt = (delivery_id: string, status: string) => ({
    delivery_id,
    route_id: 'telegram-team-route',
    status,
    expires_at: 2000000000,
    rendering: { outcome: 'rendered' },
    delivery: status === 'committed' ? { outcome: 'unknown', reason: 'provider_failure' } : null,
  });
  await page.route('**/api/event-gateway/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== new URL(baseURL!).origin) {
      boundary.unexpected.push('Off-origin gateway request');
      return route.abort();
    }
    const path = url.pathname;
    const method = request.method();
    if (path.startsWith(base)) {
      calls.push({
        method,
        path,
        key: request.headers()['idempotency-key'],
        body: request.postDataJSON(),
      });
      if (path === base && method === 'POST') {
        const id =
          calls.filter((call) => call.path === base).length === 1
            ? 'original-delivery'
            : 'separate-delivery';
        return route.fulfill({ status: 201, json: receipt(id, 'prepared') });
      }
      const id = path.slice(base.length + 1).split('/')[0]!;
      if (
        (method === 'GET' && !path.endsWith('/send')) ||
        (method === 'POST' && path.endsWith('/send'))
      )
        return route.fulfill({ json: receipt(id, 'committed') });
    }
    if (method === 'GET') {
      if (path.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
      if (path.endsWith('/v1/channel-capabilities'))
        return route.fulfill({ json: channelCapabilities });
      if (path.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
      if (path.endsWith('/activity/stream'))
        return route.fulfill({ contentType: 'text/event-stream', body: ': no activity\n\n' });
      if (path.endsWith('/v1/channels'))
        return route.fulfill({ json: { items: [teamChannel], next_cursor: null } });
    }
    boundary.unexpected.push(`${method} ${path}`);
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.setViewportSize({ width: 1600, height: 1000 });
  async function openProducer() {
    await page.goto('/event-gateway');
    await page
      .getByRole('region', { name: 'Channels' })
      .getByRole('treeitem', { name: /^Team workspace/ })
      .click();
    await page
      .getByRole('region', { name: 'Delivery map' })
      .getByRole('button', { name: 'Producer: Telegram (NyxID relay)' })
      .click();
    await page
      .getByRole('navigation', { name: 'Configuration sections' })
      .getByRole('button', { name: 'Send message', exact: true })
      .click();
  }
  await openProducer();
  const composer = page.getByRole('region', { name: 'Send to saved destination' });
  await composer.getByLabel('Message text').fill('Synthetic original message');
  await composer.getByRole('button', { name: 'Review message', exact: true }).click();
  await expect(composer.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  expect(calls).toHaveLength(1);
  await composer.getByRole('button', { name: 'Send message', exact: true }).click();
  const warning =
    'The provider reported an error. Delivery is unconfirmed; some or all of the message may have been sent.';
  await expect(composer.getByRole('status')).toHaveText(warning);
  for (const count of [3, 4]) {
    await composer.getByRole('button', { name: 'Check delivery status', exact: true }).click();
    await expect(
      composer.getByRole('button', { name: 'Check delivery status', exact: true }),
    ).toBeEnabled();
    expect(calls).toHaveLength(count);
    await expect(composer.getByRole('status')).toHaveText(warning);
  }
  await openProducer();
  await expect(composer.getByRole('status')).toHaveText(warning);
  expect(calls.map((call) => call.method)).toEqual(['POST', 'POST', 'GET', 'GET']);
  const start = composer.getByRole('button', { name: 'Start a separate message', exact: true });
  await expect(start).toBeDisabled();
  await composer.getByRole('checkbox', { name: /I understand/ }).check();
  await start.click();
  await expect(composer.getByLabel('Message text')).toHaveValue('');
  const previous = composer.getByRole('region', { name: 'Previous unconfirmed delivery' });
  await expect(previous.getByRole('status')).toHaveText(warning);
  await expect(previous).toContainText('Synthetic original message');
  await expect(previous).toContainText('original-delivery');
  expect(calls).toHaveLength(4);
  await previous.scrollIntoViewIfNeeded();
  await expect(previous.getByRole('status')).toBeVisible();
  await page.screenshot({
    path: info.outputPath('outbound-retained-provider-failure.png'),
    animations: 'disabled',
  });
  await openProducer();
  await expect(previous.getByRole('status')).toHaveText(warning);
  await previous.getByRole('button', { name: 'Check previous delivery status' }).click();
  await expect(
    previous.getByRole('button', { name: 'Check previous delivery status' }),
  ).toBeEnabled();
  expect(calls[4]).toMatchObject({ method: 'GET', path: `${base}/original-delivery` });
  await composer.getByLabel('Message text').fill('Synthetic separate message');
  expect(calls).toHaveLength(5);
  await composer.getByRole('button', { name: 'Review message', exact: true }).click();
  await expect(composer.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  expect(calls).toHaveLength(6);
  expect(calls[5]?.key).toBeTruthy();
  expect(calls[5]?.key).not.toBe(calls[0]?.key);
  expect(calls[5]?.body).toMatchObject({ message: { text: 'Synthetic separate message' } });
  await composer.getByRole('button', { name: 'Send message', exact: true }).click();
  await expect(
    composer.getByRole('button', { name: 'Check delivery status', exact: true }),
  ).toBeEnabled();
  expect(calls[6]).toMatchObject({
    method: 'POST',
    path: `${base}/separate-delivery/send`,
    body: { text: 'Synthetic separate message' },
  });
  const journals = await page.evaluate(() =>
    Object.entries(sessionStorage)
      .filter(([key]) => key.startsWith('cma:channel-outbound:'))
      .map(([key, value]) => ({ key, saved: JSON.parse(value) })),
  );
  expect(journals).toHaveLength(2);
  expect(journals.find((item) => item.key.endsWith(':previous'))?.saved.intent.draft.key).toBe(
    calls[0]?.key,
  );
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
