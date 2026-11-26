import { expect, test } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { commandStatus } from './agent-v2-command.fixture';

for (const theme of ['light', 'dark'] as const) {
  test(`LOCAL ${theme}: an automatic matching receipt dismisses only the resolved send error`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    const writes: { key: string | undefined; body: unknown }[] = [];
    const lookups: typeof writes = [];
    let release!: () => void;
    const receipt = new Promise<void>((resolve) => {
      release = resolve;
    });
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/responses', async (route) => {
        const request = route.request();
        writes.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
        await route.abort('connectionfailed');
      });
      await page.route('**/api/v2/agents/agt_contract/responses/receipt', async (route) => {
        const request = route.request();
        lookups.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
        await receipt;
        await route.fulfill({ json: commandStatus(fixture.started) });
      });
      await page.goto('/agents/agt_contract');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const send = page.getByRole('button', { name: 'Send message', exact: true });
      const text = 'Keep this exact request';
      await field.fill(text);
      await send.click();
      const error = page.getByText(
        'CMA could not be reached. Check your connection, then try again.',
        { exact: true },
      );
      await expect(error).toBeVisible();
      await expect.poll(() => lookups.length).toBe(1);
      await expect(field).toHaveValue(text);
      await expect(send).toBeDisabled();
      expect(writes).toHaveLength(1);
      expect(writes[0]!.key).toBeTruthy();
      expect(lookups).toEqual(writes);
      await page.screenshot({
        path: info.outputPath(`receipt-awaiting-${theme}.png`),
        animations: 'disabled',
      });
      release();
      await expect(field).toHaveValue('');
      await expect(error).toHaveCount(0);
      await expect(page.getByText('Awaiting confirmation', { exact: true })).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Recover pending request' })).toHaveCount(0);
      const message = page.locator('.message--user').filter({ hasText: text });
      await expect(message).toHaveCount(1);
      await expect(message).toContainText('Sent');
      expect(writes).toHaveLength(1);
      expect(lookups).toEqual(writes);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
      await page.screenshot({
        path: info.outputPath(`receipt-accepted-${theme}.png`),
        animations: 'disabled',
      });
    } finally {
      release();
      await fixture.close(page);
    }
  });
}
