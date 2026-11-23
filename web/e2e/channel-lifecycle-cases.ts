import { expect, test, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { channelCapabilities, gatewaySession, teamChannel } from '../build/fixtures/eventGateway';
import type { ChannelListItem } from '../src/services/channelTypes';

const record = (ready: boolean, version = 3): ChannelListItem => ({
  ...teamChannel,
  version,
  definition: { ...teamChannel.definition, creation: { ready } },
});
async function setup(
  page: Page,
  baseURL: string,
  initial = record(false),
  second?: ChannelListItem,
) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const state = { row: initial, recovered: record(false), readStatus: 200, deleteStatus: 409 };
  const writes: { method: string; path: string; body: string | null; key?: string }[] = [];
  await page.route('**/api/event-gateway/**', (route) => {
    const request = route.request(),
      url = new URL(request.url()),
      path = url.pathname;
    if (request.method() !== 'GET') {
      writes.push({
        method: request.method(),
        path: path + url.search,
        body: request.postData(),
        key: request.headers()['idempotency-key'],
      });
      return route.fulfill({
        status: request.method() === 'DELETE' ? state.deleteStatus : 503,
        json: { detail: 'Local result unconfirmed' },
      });
    }
    if (path.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
    if (path.endsWith('/v1/channel-capabilities'))
      return route.fulfill({ json: channelCapabilities });
    if (path.endsWith('/v1/channels'))
      return route.fulfill({
        status: state.readStatus,
        json: url.searchParams.has('after')
          ? { items: second ? [second] : [], next_cursor: null }
          : { items: [state.row], next_cursor: second ? 'next-page' : null },
      });
    if (path.includes('/activity/stream'))
      return route.fulfill({
        contentType: 'text/event-stream',
        body: 'event: activity\ndata: {"items":[],"has_more":false}\n\n',
      });
    if (path.includes('/channel-records/') || path.endsWith(`/channels/${teamChannel.channel_id}`))
      return route.fulfill({ json: state.recovered });
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.goto('/event-gateway');
  const build = await page.request.get('/build-info.json');
  expect(build.ok()).toBe(true);
  const identity = (await build.json()) as { revision: string };
  const expectedRevision = process.env.CMA_EXPECTED_BUILD_SHA ?? process.env.GITHUB_SHA;
  if (expectedRevision) expect(identity.revision).toBe(expectedRevision);
  await test.info().attach('served-build-identity', {
    body: JSON.stringify(identity),
    contentType: 'application/json',
  });
  const row = page
    .getByRole('treeitem', { includeHidden: true })
    .filter({ hasText: 'Team workspace' });
  await expect(row).toBeAttached();
  if (!(await row.isVisible())) await page.getByRole('button', { name: 'Show navigation' }).click();
  await row.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'Team workspace', exact: true })).toBeVisible();
  return { state, writes, boundary };
}

export function channelLifecycleCases() {
  test('LOCAL paginated channel refresh retains the exact uncertain request and requires fresh record evidence', async ({
    page,
    baseURL,
  }) => {
    const second = {
      ...record(true),
      record_id: 'second-record',
      channel_id: 'second-channel',
      definition: {
        ...record(true).definition,
        name: 'Second page',
        owner: { kind: 'human', subject: 'local-owner' },
      },
    };
    const { writes, boundary } = await setup(page, baseURL!, record(true), second);
    await page.getByRole('button', { name: 'Load more channels', exact: true }).click();
    await page.getByRole('treeitem').filter({ hasText: 'Second page' }).click();
    await page.getByRole('button', { name: /^Channel: Second page/ }).click();
    const dialog = page.getByRole('dialog', { name: 'Channel settings' });
    const input = dialog.getByLabel('Channel name', { exact: true });
    const originalInput = await input.elementHandle();
    await input.fill('Retained second-page draft');
    await dialog.getByRole('button', { name: 'Save channel', exact: true }).click();
    await expect(
      dialog.getByRole('button', { name: 'Retry same request', exact: true }),
    ).toBeEnabled();
    expect(writes).toHaveLength(1);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await page.getByRole('button', { name: 'Refresh channel status', exact: true }).click();
    await expect(
      page.getByRole('button', { name: 'Load more channels', exact: true }),
    ).toBeEnabled();
    await expect(page.getByRole('status').filter({ hasText: 'status stale' })).toBeVisible();
    expect(await originalInput!.evaluate((element) => element.isConnected)).toBe(true);
    await page.getByRole('button', { name: /^Channel: Second page/ }).click();
    await expect(input).toHaveValue('Retained second-page draft');
    await expect(
      dialog.getByRole('button', { name: 'Retry same request', exact: true }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole('button', { name: 'Delete channel', exact: true }),
    ).toBeDisabled();
    await dialog
      .locator('form')
      .first()
      .evaluate((form) =>
        form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })),
      );
    expect(writes).toHaveLength(1);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await page.getByRole('button', { name: 'Load more channels', exact: true }).click();
    await expect(page.getByRole('treeitem').filter({ hasText: 'Second page' })).toBeVisible();
    await page.getByRole('button', { name: /^Channel: Second page/ }).click();
    await expect(
      dialog.getByRole('button', { name: 'Retry same request', exact: true }),
    ).toBeEnabled();
    expect(await originalInput!.evaluate((element) => element.isConnected)).toBe(true);
    await expect(input).toHaveValue('Retained second-page draft');
    expect(writes).toHaveLength(1);
    await dialog.getByRole('button', { name: 'Retry same request', exact: true }).click();
    await expect.poll(() => writes.length).toBe(2);
    expect(writes[1]).toEqual(writes[0]);
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
  });

  for (const theme of ['light', 'dark'] as const) {
    test(`LOCAL lifecycle readiness preserves drafts and rejects writes in ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      await page.setViewportSize({ width: 390, height: 1000 });
      const { state, writes, boundary } = await setup(page, baseURL!);
      for (const width of [390, 1440]) {
        await page.setViewportSize({ width, height: 1000 });
        await expect(
          page.getByRole('status').filter({ hasText: 'Creation incomplete' }),
        ).toBeVisible();
        await expect(page.getByRole('button', { name: 'Add producer', exact: true })).toHaveCount(
          0,
        );
        const overflow = await page.evaluate(
          () => document.documentElement.scrollWidth > innerWidth,
        );
        expect(overflow).toBe(false);
        const notice = page.getByRole('status').filter({ hasText: 'Creation incomplete' });
        await expect
          .poll(() =>
            notice.evaluate((element) => {
              const bounds = element.getBoundingClientRect();
              return bounds.left >= 0 && bounds.right <= innerWidth && bounds.width > 200;
            }),
          )
          .toBe(true);
        await page.screenshot({
          path: info.outputPath(`lifecycle-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
      const dialog = page.getByRole('dialog', { name: 'Channel settings' });
      await expect(
        dialog.getByRole('button', { name: 'Save channel', exact: true }),
      ).toBeDisabled();
      await expect(
        dialog.getByRole('button', { name: 'Send test event', exact: true }),
      ).toBeDisabled();
      await dialog
        .locator('form')
        .first()
        .evaluate((form) =>
          form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })),
        );
      expect(writes).toEqual([]);
      await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      await page
        .getByRole('button', { name: 'Producer: Telegram (NyxID relay)', exact: true })
        .click();
      const producer = page.getByRole('dialog');
      await expect(
        producer.getByRole('button', { name: 'Save producer', exact: true }),
      ).toBeDisabled();
      await expect(
        producer.getByRole('button', { name: 'Check configuration', exact: true }),
      ).toBeDisabled();
      await expect(
        producer.getByRole('button', { name: 'Review message', exact: true }),
      ).toBeDisabled();
      await producer
        .locator('form')
        .first()
        .evaluate((form) =>
          form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })),
        );
      expect(writes).toEqual([]);
      await producer.getByRole('button', { name: 'Close', exact: true }).click();
      state.row = record(true, 4);
      await page.getByRole('button', { name: 'Refresh channel status', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: 'Setup complete' })).toBeVisible();
      await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
      await dialog.getByLabel('Channel name', { exact: true }).fill('Retained lifecycle draft');
      await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      state.readStatus = 503;
      await page.getByRole('button', { name: 'Refresh channel status', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: 'status stale' })).toBeVisible();
      await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
      await expect(dialog.getByLabel('Channel name', { exact: true })).toHaveValue(
        'Retained lifecycle draft',
      );
      await expect(
        dialog.getByRole('button', { name: 'Save channel', exact: true }),
      ).toBeDisabled();
      await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      state.readStatus = 200;
      state.row = record(true, 5);
      await page.getByRole('button', { name: 'Refresh channel status', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: 'Setup complete' })).toBeVisible();
      await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
      await expect(dialog.getByLabel('Channel name', { exact: true })).toHaveValue(
        'Retained lifecycle draft',
      );
      await dialog.getByRole('button', { name: 'Save channel', exact: true }).click();
      await expect(
        dialog.getByRole('button', { name: 'Retry same request', exact: true }),
      ).toBeEnabled();
      expect(writes).toHaveLength(1);
      await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      state.row = record(false, 6);
      await page.getByRole('button', { name: 'Refresh channel status', exact: true }).click();
      await expect(
        page.getByRole('status').filter({ hasText: 'Creation incomplete' }),
      ).toBeVisible();
      await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
      await expect(
        dialog.getByRole('button', { name: 'Retry same request', exact: true }),
      ).toBeDisabled();
      expect(writes).toHaveLength(1);
      await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      state.row = record(true, 7);
      await page.getByRole('button', { name: 'Refresh channel status', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: 'Setup complete' })).toBeVisible();
      await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
      await dialog.getByRole('button', { name: 'Retry same request', exact: true }).click();
      await expect.poll(() => writes.length).toBe(2);
      expect(writes[1]).toEqual(writes[0]);
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
    });
  }

  test('LOCAL lifecycle recovered incomplete address cannot write and cleanup refusal retains identity', async ({
    page,
    baseURL,
  }) => {
    const { writes, boundary } = await setup(page, baseURL!, {
      ...record(false),
      channel_id: undefined,
      endpoints: undefined,
    });
    await page.getByRole('button', { name: /^Channel: Team workspace/ }).click();
    const dialog = page.getByRole('dialog', { name: 'Channel settings' });
    await expect(dialog.getByRole('button', { name: 'Save channel', exact: true })).toBeDisabled();
    await expect(
      dialog.getByRole('button', { name: 'Send test event', exact: true }),
    ).toBeDisabled();
    await dialog.getByRole('button', { name: 'Delete channel', exact: true }).click();
    await dialog.getByRole('button', { name: 'Confirm delete', exact: true }).click();
    await expect(dialog.getByRole('alert')).toHaveText('Local result unconfirmed');
    expect(writes).toEqual([
      {
        method: 'DELETE',
        path: '/api/event-gateway/v1/channels/channel-local?expected_version=3',
        body: null,
        key: undefined,
      },
    ]);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Team workspace', exact: true })).toBeVisible();
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
  });
}
