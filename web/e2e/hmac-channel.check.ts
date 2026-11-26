import { test, expect, type Page } from '@playwright/test';
import { setup } from './channel-setup.fixture';

const secret = Buffer.from('synthetic channel signing material').toString('base64');

async function configureCustom(page: Page) {
  await page.getByLabel('Channel name', { exact: true }).fill('Synthetic HMAC channel');
  await page.getByLabel('Provider agent key', { exact: true }).fill('synthetic-provider');
  await page.getByRole('button', { name: 'Producer 1 connection type', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Custom event', exact: true }).click();
  await page.getByLabel('HMAC key ID', { exact: true }).fill('events');
  await page.getByRole('button', { name: 'Add HMAC credential', exact: true }).click();
}

for (const width of [390, 1440]) {
  test(`LOCAL ${width}px initial HMAC setup blocks invalid references and secrets before transport`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 844 });
    const writes = await setup(page, baseURL!);
    await configureCustom(page);
    await expect(page.getByLabel('Replace HMAC credentials', { exact: true })).toHaveCount(0);
    await expect(page.getByText(/replaces the whole HMAC key list/)).toHaveCount(0);
    await expect(page.getByText(/Set the initial HMAC credentials/)).toBeVisible();
    await page.getByLabel('HMAC 1 key ID', { exact: true }).fill('different');
    await page.getByLabel('HMAC 1 secret', { exact: true }).fill('not base64!');
    const create = page.getByRole('button', { name: 'Create channel', exact: true });
    const sourceId = page.getByLabel('HMAC key ID', { exact: true });
    const secretInput = page.getByLabel('HMAC 1 secret', { exact: true });
    await create.click();
    await expect(sourceId).toBeFocused();
    await expect(sourceId).toHaveAccessibleDescription(/exact key ID/);
    expect(writes).toHaveLength(0);

    await page.getByLabel('HMAC 1 key ID', { exact: true }).fill('events');
    await create.click();
    await expect(secretInput).toBeFocused();
    await expect(secretInput).toHaveAccessibleDescription(/standard base64/i);
    expect(writes).toHaveLength(0);
    await expect(page.getByText(/\/credentials\/hmac_secrets/)).toHaveCount(0);

    await secretInput.fill(secret);
    await expect(secretInput).not.toHaveAttribute('aria-invalid', 'true');
    await create.click();
    await expect(
      page.getByRole('button', { name: 'Retry same request', exact: true }),
    ).toBeVisible();
    expect(writes).toHaveLength(1);
    const body = JSON.parse(writes[0]!.body!);
    expect(body.sources[0]).toEqual({ type: 'custom', auth: { type: 'hmac', key_id: 'events' } });
    expect(body.partition.key).toBe('event_subject');
    expect(body.profile.environment).toEqual({
      size: { cpu_cores: 1, memory_gib: 2 },
      workspace: { type: 'directory', name: 'workspace' },
    });
    expect(body.credentials.hmac_secrets).toEqual([{ key_id: 'events', secret }]);
    expect(body.credentials).not.toHaveProperty('replace_hmac');
    await page.getByRole('button', { name: 'Retry same request', exact: true }).click();
    await expect.poll(() => writes.length).toBe(2);
    expect(writes[1]!.body).toBe(writes[0]!.body);
    expect(writes[1]!.key).toBe(writes[0]!.key);
  });
}

test('LOCAL backend HMAC validation names and focuses the field without a JSON pointer', async ({
  page,
  baseURL,
}) => {
  await setup(page, baseURL!);
  await configureCustom(page);
  await page.getByLabel('HMAC 1 key ID', { exact: true }).fill('events');
  await page.getByLabel('HMAC 1 secret', { exact: true }).fill(secret);
  await page.route('**/api/event-gateway/v1/channels', (route) =>
    route.request().method() === 'POST'
      ? route.fulfill({
          status: 400,
          json: { detail: '/credentials/hmac_secrets/0/secret: must be base64' },
        })
      : route.fallback(),
  );
  await page.getByRole('button', { name: 'Create channel', exact: true }).click();
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('HMAC 1 secret');
  await expect(alert).toContainText('must be base64');
  await expect(alert).not.toContainText('/credentials/');
  await expect(alert).not.toContainText(secret);
  await expect(page.getByLabel('HMAC 1 secret', { exact: true })).toBeFocused();
});
