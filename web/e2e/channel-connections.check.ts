import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';
import type { Channel } from '../src/services/ChannelEditor';

test('LOCAL saved connections stay read-only and exact across saves, reopen and channel switches', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const channels: Channel[] = [
    {
      ...teamChannel,
      channel_id: 'synthetic-discord',
      record_id: 'synthetic-discord-record',
      definition: {
        ...teamChannel.definition,
        name: 'Direct Discord',
        sources: [{ type: 'discord', bot_id: '123', admission: { type: 'all' } }],
      },
    },
    {
      ...teamChannel,
      channel_id: 'synthetic-telegram',
      record_id: 'synthetic-telegram-record',
      definition: {
        ...teamChannel.definition,
        name: 'Managed Telegram',
        connection_ref: 'Connection_Exact.Synthetic-123',
        sources: [{ type: 'telegram', bot_id: '456', admission: { type: 'all' } }],
      },
    },
  ];
  const writes: { path: string; method: string; body: Record<string, unknown>; key?: string }[] =
    [];
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== baseURL) return route.fallback();
    if (url.pathname === '/auth/session') return route.fulfill({ json: cmaSession });
    if (url.pathname === '/api/v1/bots')
      return route.fulfill({ json: { items: [], next_after: null } });
    // The existing channel map reports ResizeObserver notifications through telemetry.
    if (url.pathname === '/api/v1/telemetry/browser' && request.method() === 'POST') {
      expect(request.postDataJSON()).toEqual({ kind: 'uncaught_error' });
      return route.fulfill({ status: 202, json: {} });
    }
    if (!url.pathname.startsWith('/api/event-gateway/')) return route.fallback();
    if (request.method() === 'GET') {
      if (url.pathname.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
      if (url.pathname.endsWith('/v1/channel-capabilities'))
        return route.fulfill({ json: channelCapabilities });
      if (url.pathname.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
      if (url.pathname.endsWith('/v1/channels'))
        return route.fulfill({ json: { items: channels, next_cursor: null } });
      if (url.pathname.endsWith('/activity/stream'))
        return route.fulfill({ contentType: 'text/event-stream', body: ': local fixture\n\n' });
    }
    const channel = channels.find(
      (item) => url.pathname === `/api/event-gateway/v1/channels/${item.channel_id}`,
    );
    if (request.method() === 'PUT' && channel) {
      const body = request.postDataJSON() as Record<string, unknown>;
      writes.push({
        path: url.pathname,
        method: request.method(),
        body,
        key: request.headers()['idempotency-key'],
      });
      expect(body.expected_version).toBe(channel.version);
      expect(body.connection_ref).toBe(channel.definition.connection_ref);
      expect(body.sources).toEqual(channel.definition.sources);
      expect(body).not.toHaveProperty('credentials');
      channel.definition = { ...channel.definition, name: String(body.name) };
      channel.version++;
      return route.fulfill({ json: channel });
    }
    boundary.unexpected.push(`${request.method()} ${url.pathname}`);
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/event-gateway');
  const open = async (name: string) => {
    await page
      .getByRole('region', { name: 'Channels', exact: true })
      .getByRole('treeitem', { name: new RegExp(`^${name}`) })
      .click();
    await page
      .getByRole('region', { name: 'Delivery map' })
      .getByRole('button', { name: `Channel: ${name}` })
      .click();
  };
  const dialog = page.getByRole('dialog');
  const reference = dialog.getByRole('textbox', { name: 'Connection reference' });
  for (const [index, channel] of channels.entries()) {
    await open(channel.definition.name);
    const savedValue = channel.definition.connection_ref || 'No shared connection';
    await expect(reference).toHaveValue(String(savedValue));
    await expect(reference).not.toBeEditable();
    await expect(reference).toHaveAccessibleDescription(
      "This channel's saved CMA-managed connection. When empty, the channel uses its producer settings. The reference cannot be added, removed, or replaced after channel creation. To use a different managed connection, create a new channel.",
    );
    await expect(dialog.getByText(/CMA-managed Telegram connection reference/)).toHaveCount(0);
    await reference.click();
    await reference.press('ControlOrMeta+a');
    await reference.press('Backspace');
    await reference.pressSequentially('replacement');
    await expect(reference).toHaveValue(String(savedValue));
    const renamed = `${channel.definition.name} renamed`;
    await dialog.getByLabel('Channel name', { exact: true }).fill(renamed);
    expect(writes).toHaveLength(index);
    await dialog.screenshot({ path: info.outputPath(`connection-${index}.png`) });
    await dialog.getByRole('button', { name: 'Save channel', exact: true }).click();
    await expect(dialog.getByText('Changes saved.', { exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await open(renamed);
    await expect(reference).toHaveValue(String(savedValue));
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  }
  await page.reload();
  for (const channel of channels) {
    await open(channel.definition.name);
    await expect(reference).toHaveValue(
      String(channel.definition.connection_ref || 'No shared connection'),
    );
    await expect(reference).not.toBeEditable();
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
  }
  expect(writes).toHaveLength(2);
  expect(writes.every((write) => write.key && write.method === 'PUT')).toBe(true);
  expect(boundary.errors).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
