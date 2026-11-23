import { test, expect, type Page } from '@playwright/test';
import { setup } from './channel-setup.fixture';
import { registerRedesignChannelCases } from './redesign-channel.cases';

registerRedesignChannelCases();

async function completeRequiredFields(page: Page) {
  await page.getByLabel('Channel name', { exact: true }).fill('Validation draft');
  await page.getByLabel('NyxID issuer', { exact: true }).fill('https://relay.example.test');
  await page.getByLabel('Signing key ID', { exact: true }).fill('local-signing');
  await page.getByLabel('Route IDs 1', { exact: true }).fill('local-route');
  await page.getByLabel('Provider agent key', { exact: true }).fill('local-provider-fixture');
}

for (const width of [390, 1440]) {
  for (const kind of ['allowlist', 'second producer scoped', 'managed connection'] as const) {
    test(`LOCAL ${width}px ${kind} refusal focuses only its admission policy`, async ({
      page,
      baseURL,
    }) => {
      await page.setViewportSize({ width, height: 1000 });
      await setup(page, baseURL!);
      await completeRequiredFields(page);
      const second = kind === 'second producer scoped';
      const managed = kind === 'managed connection';
      if (managed) {
        await page
          .getByLabel('Use an existing CMA-managed Telegram connection', { exact: true })
          .check();
        await page.getByRole('button', { name: 'Saved Telegram connection', exact: true }).click();
        await page.getByRole('menuitemradio', { name: 'Owned Telegram bot', exact: true }).click();
      } else if (second) {
        await page.getByRole('button', { name: 'Add producer', exact: true }).click();
        await page.getByRole('button', { name: 'Producer 2 connection type', exact: true }).click();
        await page
          .getByRole('menuitemradio', { name: 'NyxID messaging connection', exact: true })
          .click();
        const producer = page.getByRole('group', { name: 'Producer 2', exact: true });
        await producer
          .getByLabel('NyxID issuer', { exact: true })
          .fill('https://second.example.test');
        await producer.getByLabel('Signing key ID', { exact: true }).fill('second-signing');
        await producer.getByLabel('Route IDs 1', { exact: true }).fill('second-route');
      }
      const policy = page.getByRole('group', {
        name: managed ? 'Connection admission' : `Producer ${second ? 2 : 1} access`,
        exact: true,
      });
      if (!managed) {
        await policy.getByRole('button', { name: 'Admission policy', exact: true }).click();
        await page
          .getByRole('menuitemradio', {
            name: second ? 'Sender, chat and group rules' : 'Allowed senders',
            exact: true,
          })
          .click();
      }
      if (second) {
        await policy.getByRole('button', { name: 'Senders policy', exact: true }).click();
        await page.getByRole('menuitemradio', { name: 'allowlist', exact: true }).click();
      }
      const label = second ? 'Senders IDs' : 'Allowed sender IDs';
      for (const index of [1, 2]) {
        await policy.getByRole('button', { name: `Add ${label}`, exact: true }).click();
        await policy.getByLabel(`${label} ${index}`, { exact: true }).fill('duplicate-sender');
      }
      const connections = page
        .locator('details')
        .filter({ has: page.locator('summary', { hasText: /^Advanced connection settings$/ }) });
      for (const details of await connections.all()) {
        await details.locator('summary').click();
        await expect(details).not.toHaveAttribute('open');
      }
      const requests: string[] = [];
      const path = `/sources/${second ? 1 : 0}/admission${second ? '/senders' : ''}`;
      await page.route('**/api/event-gateway/v1/channels', (route) => {
        if (route.request().method() !== 'POST') return route.fallback();
        requests.push(route.request().postData()!);
        return route.fulfill({ status: 400, json: { detail: `${path}: values must be unique` } });
      });
      await page.getByRole('button', { name: 'Create channel', exact: true }).click();
      const target = policy.getByLabel(`${label} 1`, { exact: true });
      await expect(target).toBeFocused();
      await expect(target).toHaveAccessibleDescription(/values must be unique/);
      await page.getByRole('button', { name: 'Review channel fields', exact: true }).click();
      await expect(target).toBeFocused();
      for (const details of await connections.all())
        await expect(details).not.toHaveAttribute('open');
      await expect(
        page.getByText('Advanced Agent tools and environment', { exact: true }).locator('..'),
      ).not.toHaveAttribute('open');
      for (const index of [1, 2])
        await expect(policy.getByLabel(`${label} ${index}`, { exact: true })).toHaveValue(
          'duplicate-sender',
        );
      await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue(
        'Validation draft',
      );
      expect(requests).toHaveLength(1);
      const body = JSON.parse(requests[0]!);
      const admission = managed
        ? body.connection_admission
        : body.sources[second ? 1 : 0].admission;
      expect(admission.type).toBe(second ? 'scoped' : 'allowlist');
      expect(second ? admission.senders.ids : admission.sender_ids).toEqual([
        'duplicate-sender',
        'duplicate-sender',
      ]);
      if (second) expect(body.sources[0].admission).toEqual({ type: 'all' });
    });
  }
}

test('LOCAL a rejected second producer reveals only its own connection fields', async ({
  page,
  baseURL,
}) => {
  await setup(page, baseURL!);
  await completeRequiredFields(page);
  await page.getByRole('button', { name: 'Add producer', exact: true }).click();
  await page.getByRole('button', { name: 'Producer 2 connection type', exact: true }).click();
  await page
    .getByRole('menuitemradio', { name: 'NyxID messaging connection', exact: true })
    .click();
  const producer = page.getByRole('group', { name: 'Producer 2', exact: true });
  await producer.getByLabel('NyxID issuer', { exact: true }).fill('https://second.example.test');
  await producer.getByLabel('Signing key ID', { exact: true }).fill('second-signing');
  await producer.getByLabel('Route IDs 1', { exact: true }).fill('second-route');
  await producer.getByRole('button', { name: 'Add Route IDs', exact: true }).click();
  await producer.getByLabel('Route IDs 2', { exact: true }).fill('second-route');
  const first = page.getByRole('group', { name: 'Producer 1', exact: true });
  await first.locator('summary').click();
  await producer.locator('summary').click();
  await page.route('**/api/event-gateway/v1/channels', (route) =>
    route.request().method() === 'POST'
      ? route.fulfill({
          status: 400,
          json: { detail: '/sources/1/route_ids: values must be unique' },
        })
      : route.fallback(),
  );
  await page.getByRole('button', { name: 'Create channel', exact: true }).click();
  await expect(producer.getByLabel('Route IDs 1', { exact: true })).toBeFocused();
  await expect(producer.getByLabel('Route IDs 2', { exact: true })).toHaveValue('second-route');
  await expect(first.locator('details')).not.toHaveAttribute('open');
  await expect(first.getByLabel('Route IDs 1', { exact: true })).toHaveValue('local-route');
});

for (const width of [390, 1440]) {
  test(`LOCAL ${width}px duplicate routes reveal the rejected producer and retain the corrected draft`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 1000 });
    await setup(page, baseURL!);
    await completeRequiredFields(page);
    const requests: { body: string | null; key: string | undefined }[] = [];
    await page.route('**/api/event-gateway/v1/channels', (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      requests.push({
        body: route.request().postData(),
        key: route.request().headers()['idempotency-key'],
      });
      return route.fulfill({
        status: requests.length === 1 ? 400 : 503,
        json: {
          detail:
            requests.length === 1
              ? '/sources/0/route_ids: values must be unique'
              : 'Local result unconfirmed',
        },
      });
    });
    await page.getByRole('button', { name: 'Add Route IDs', exact: true }).click();
    await page.getByLabel('Route IDs 2', { exact: true }).fill('local-route');
    const advanced = page
      .locator('details')
      .filter({ has: page.locator('summary', { hasText: /^Advanced connection settings$/ }) });
    await advanced.locator('summary').click();
    await expect(advanced).not.toHaveAttribute('open');
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('values must be unique');
    await expect(advanced).toHaveAttribute('open', '');
    const first = page.getByLabel('Route IDs 1', { exact: true });
    await expect(first).toBeVisible();
    await expect(first).toBeFocused();
    await expect(first).toHaveAccessibleDescription(/values must be unique/);
    await expect(page.getByLabel('Route IDs 2', { exact: true })).toHaveValue('local-route');
    await page.getByLabel('Route IDs 2', { exact: true }).fill('corrected-route');
    await advanced.locator('summary').click();
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Local result unconfirmed');
    expect(requests).toHaveLength(2);
    expect(JSON.parse(requests[1]!.body!).sources[0].route_ids).toEqual([
      'local-route',
      'corrected-route',
    ]);
    expect(requests[1]!.key).not.toBe(requests[0]!.key);
    await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue('Validation draft');
    await page.getByRole('button', { name: 'Retry same request', exact: true }).click();
    await expect.poll(() => requests.length).toBe(3);
    expect(requests[2]).toEqual(requests[1]);
  });

  test(`LOCAL ${width}px profile errors reveal the exact nested expert field`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 1000 });
    await setup(page, baseURL!);
    await completeRequiredFields(page);
    await page.getByText('Advanced Agent tools and environment', { exact: true }).click();
    await page.getByRole('button', { name: 'Add MCP server', exact: true }).click();
    await page.getByLabel('Server 1 URL', { exact: true }).fill('http://tools.example.test');
    await page.getByText('Advanced Agent tools and environment', { exact: true }).click();
    await page.route('**/api/event-gateway/v1/channels', (route) =>
      route.request().method() === 'POST'
        ? route.fulfill({
            status: 400,
            json: { detail: '/profile/context/mcp_servers/0/url: must use https' },
          })
        : route.fallback(),
    );
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByLabel('Server 1 URL', { exact: true })).toBeVisible();
    await expect(page.getByLabel('Server 1 URL', { exact: true })).toBeFocused();
    await expect(page.getByLabel('Server 1 URL', { exact: true })).toHaveValue(
      'http://tools.example.test',
    );
  });

  test(`LOCAL ${width}px unmapped refusals keep summary focus and preserve unrelated settings`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 1000 });
    await setup(page, baseURL!);
    await completeRequiredFields(page);
    let requests = 0;
    await page.route('**/api/event-gateway/v1/channels', (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      requests++;
      return route.fulfill({
        status: 400,
        json: { detail: 'Provider refused this configuration.' },
      });
    });
    await page
      .getByLabel('Use an existing CMA-managed Telegram connection', { exact: true })
      .check();
    const expandedSections = () =>
      page
        .locator('details')
        .evaluateAll((elements) => elements.map((element) => (element as HTMLDetailsElement).open));
    const managedSections = await expandedSections();
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText(
      'Choose a ready CMA-managed Telegram connection.',
    );
    await expect(page.getByRole('alert')).toBeFocused();
    expect(requests).toBe(0);
    await page.getByRole('button', { name: 'Review channel fields', exact: true }).click();
    await expect(page.getByRole('alert')).toBeFocused();
    await expect(page.getByLabel('Channel name', { exact: true })).not.toBeFocused();
    expect(await expandedSections()).toEqual(managedSections);
    expect(requests).toBe(0);
    await page
      .getByLabel('Use an existing CMA-managed Telegram connection', { exact: true })
      .uncheck();
    await expect(page.getByLabel('NyxID issuer', { exact: true })).not.toBeVisible();
    const manualSections = await expandedSections();
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByRole('alert')).toBeFocused();
    await expect(page.getByRole('alert')).toContainText('Provider refused this configuration.');
    await page.getByRole('button', { name: 'Review channel fields', exact: true }).click();
    await expect(page.getByRole('alert')).toBeFocused();
    await expect(page.getByLabel('NyxID issuer', { exact: true })).not.toBeVisible();
    await expect(page.getByLabel('Channel name', { exact: true })).not.toBeFocused();
    expect(await expandedSections()).toEqual(manualSections);
    await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue('Validation draft');
    expect(requests).toBe(1);
  });
}
