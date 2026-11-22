import { test, expect } from '@playwright/test';
import { setup } from './channel-setup.fixture';

for (const width of [390, 1440]) {
  test(`LOCAL channel setup at ${width}px exposes required fields and preserves the original create request`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    const writes = await setup(page, baseURL!);
    const editor = page.locator('.channel-node-editor');
    const groups = editor.locator('form > fieldset > fieldset');
    await expect(groups.locator(':scope > legend')).toHaveText([
      'Connection',
      'Agent behavior',
      'Access & reply policy',
      'Review',
    ]);
    const advanced = page
      .locator('details')
      .filter({ has: page.locator('summary', { hasText: /^Advanced connection settings$/ }) });
    await expect(advanced).toHaveAttribute('open', '');
    await expect(page.getByLabel('NyxID issuer', { exact: true })).toHaveAccessibleDescription(
      /Required.*existing NyxID relay route/,
    );
    await expect(
      page.getByRole('button', { name: 'Messaging platform', exact: true }),
    ).toBeInViewport();
    await page.screenshot({ path: info.outputPath(`channel-connection-${width}.png`) });
    await page.getByLabel('Channel name', { exact: true }).fill('Local review channel');
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByLabel('NyxID issuer', { exact: true })).toBeFocused();
    expect(writes).toEqual([]);
    await page.getByLabel('NyxID issuer', { exact: true }).fill('https://relay.example.test');
    await page.getByLabel('Signing key ID', { exact: true }).fill('local-signing');
    await page.getByLabel('Route IDs 1', { exact: true }).fill('local-route');
    const summary = advanced.locator('summary');
    await summary.focus();
    await page.keyboard.press('Enter');
    await expect(advanced).not.toHaveAttribute('open');
    await expect(
      page.getByText('Issuer: https://relay.example.test', { exact: false }),
    ).toBeVisible();
    const access = page.getByRole('group', { name: 'Access & reply policy', exact: true });
    await expect(
      access.getByRole('button', { name: 'Admission policy', exact: true }),
    ).toBeVisible();
    await access.getByRole('button', { name: 'Admission policy', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'Allowed senders', exact: true }).click();
    await access.getByRole('button', { name: 'Add Allowed sender IDs', exact: true }).click();
    await access.getByLabel('Allowed sender IDs 1', { exact: true }).fill('local-sender');
    await expect(
      access.getByRole('checkbox', { name: /^Send final answers automatically/ }),
    ).toBeChecked();
    await page.getByLabel('Provider agent key', { exact: true }).fill('local-provider-fixture');
    await summary.click();
    await expect(page.getByLabel('NyxID issuer', { exact: true })).toHaveValue(
      'https://relay.example.test',
    );
    await page.getByLabel('Signing key ID', { exact: true }).fill('');
    await summary.click();
    await expect(advanced).toHaveAttribute('open', '');
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByLabel('Signing key ID', { exact: true })).toBeFocused();
    expect(writes).toEqual([]);
    await page.getByLabel('Signing key ID', { exact: true }).fill('local-signing');
    await summary.click();
    await page.getByRole('group', { name: 'Review', exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`channel-review-${width}.png`) });
    await page.getByRole('button', { name: 'Create channel', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('Local result unconfirmed');
    expect(writes).toHaveLength(1);
    expect(JSON.parse(writes[0]!.body!)).toMatchObject({
      name: 'Local review channel',
      sources: [
        {
          issuer: 'https://relay.example.test',
          key_id: 'local-signing',
          route_ids: ['local-route'],
          admission: { type: 'allowlist', sender_ids: ['local-sender'] },
        },
      ],
      profile: { permission: { service_scope: ['chrono-llm-public', 'ornn-api'] } },
    });
    await page.getByRole('button', { name: 'Retry same request', exact: true }).click();
    await expect.poll(() => writes.length).toBe(2);
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]!.key).toBeTruthy();
    await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue(
      'Local review channel',
    );
    await expect(page.getByLabel('Channel name', { exact: true })).toBeDisabled();
  });

  test(`LOCAL channel setup at ${width}px keeps connection and platform consent explicit`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 1000 });
    const writes = await setup(page, baseURL!);
    await page.getByRole('button', { name: 'Messaging platform', exact: true }).click();
    await expect(page.getByRole('menuitemradio', { name: /slack/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await page.keyboard.press('Escape');
    await page.getByLabel('NyxID issuer', { exact: true }).fill('https://retained.example.test');
    await page
      .getByLabel('Use an existing CMA-managed Telegram connection', { exact: true })
      .check();
    const picker = page.getByRole('button', { name: 'Saved Telegram connection', exact: true });
    await expect(picker).toContainText('Choose a connection');
    await expect(
      page
        .getByRole('group', { name: 'Access & reply policy', exact: true })
        .getByRole('button', { name: 'Connection admission policy', exact: true }),
    ).toContainText('Allowed senders');
    await picker.click();
    await expect(page.getByRole('menuitemradio', { name: /Pending Telegram bot/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    await page.getByRole('menuitemradio', { name: 'Owned Telegram bot', exact: true }).click();
    await page
      .getByLabel('Use an existing CMA-managed Telegram connection', { exact: true })
      .uncheck();
    await expect(page.getByLabel('NyxID issuer', { exact: true })).toHaveValue(
      'https://retained.example.test',
    );
    await page
      .getByLabel('Use an existing CMA-managed Telegram connection', { exact: true })
      .check();
    await expect(picker).toContainText('Owned Telegram bot');
    expect(writes).toEqual([]);
  });
}

test('LOCAL a channel editor survives a slow CMA sign-in resolving', async ({ page, baseURL }) => {
  let signedIn!: () => void;
  const identity = new Promise<void>((resolve) => {
    signedIn = resolve;
  });
  const capabilityReads: string[] = [];
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.endsWith('/v1/channel-capabilities'))
      capabilityReads.push(request.url());
  });
  const opened = setup(page, baseURL!, identity);
  // Whatever the Gateway offers before sign-in resolves must survive it resolving.
  await expect(
    page
      .getByRole('status', { name: 'Signing you in', exact: true })
      .or(page.getByRole('button', { name: 'Create channel', exact: true })),
  ).toBeVisible();
  signedIn();
  const writes = await opened;
  await expect(
    page.getByRole('button', { name: 'Account menu for Local owner', exact: true }),
  ).toBeVisible();
  await expect(page.getByRole('button', { name: 'Create channel', exact: true })).toBeVisible();
  const name = page.getByLabel('Channel name', { exact: true });
  await name.fill('Draft across sign-in');
  await expect(name).toHaveValue('Draft across sign-in');
  expect(capabilityReads).toHaveLength(1);
  expect(writes).toEqual([]);
});
