import { expect, test } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

for (const theme of ['light', 'dark'] as const) {
  for (const failure of ['read', 'write'] as const) {
    test(`LOCAL ${theme}: a draft binding ${failure} failure refuses before dispatch and preserves explicit retry`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      try {
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        const field = page.getByRole('textbox', { name: 'Message', exact: true });
        const send = page.getByRole('button', { name: 'Send message', exact: true });
        const text = 'Keep this draft until its request can be retained';
        await field.fill(text);
        const saved = () =>
          page.evaluate(() => {
            const key = `cma:agent-draft:${JSON.stringify([location.origin, 'local-owner', 'agt_contract'])}`;
            return sessionStorage.getItem(key);
          });
        const before = await saved();
        await page.evaluate((operation) => {
          const get = Storage.prototype.getItem;
          const set = Storage.prototype.setItem;
          const fail = () => {
            throw new DOMException('Draft binding unavailable', 'QuotaExceededError');
          };
          Storage.prototype.getItem = function (key) {
            if (operation === 'read' && key.startsWith('cma:agent-draft:')) fail();
            return get.call(this, key);
          };
          Storage.prototype.setItem = function (key, value) {
            if (
              operation === 'write' &&
              key.startsWith('cma:agent-draft:') &&
              JSON.parse(value).commandKey
            )
              fail();
            return set.call(this, key, value);
          };
          window.addEventListener(
            'restore-draft-storage',
            () => {
              Storage.prototype.getItem = get;
              Storage.prototype.setItem = set;
            },
            { once: true },
          );
        }, failure);
        await send.click();
        await expect(page.getByText('Not sent', { exact: true })).toBeVisible();
        await expect(field).toHaveValue(text);
        await expect(page.getByText('Awaiting confirmation', { exact: true })).toHaveCount(0);
        await expect(page.getByRole('button', { name: 'Recover pending request' })).toHaveCount(0);
        await expect(
          page.getByText(/message was not sent because its draft could not be saved/),
        ).toBeVisible();
        expect(fixture.writes).toEqual([]);
        expect(fixture.receiptReads).toEqual([]);
        await page.screenshot({
          path: info.outputPath(`binding-${failure}-${theme}.png`),
          animations: 'disabled',
        });
        await page.evaluate(() => window.dispatchEvent(new Event('restore-draft-storage')));
        expect(await saved()).toBe(before);
        await page.getByRole('button', { name: 'Retry draft storage' }).click();
        await expect(page.getByRole('button', { name: 'Retry draft storage' })).toHaveCount(0);
        expect(fixture.writes).toEqual([]);
        expect(fixture.receiptReads).toEqual([]);
        await send.click();
        await expect.poll(() => fixture.writes.length).toBe(1);
        await expect(field).toHaveValue('');
        await expect(page.getByText('Sent', { exact: true })).toBeVisible();
        await expect(page.getByText('Not sent', { exact: true })).toHaveCount(0);
        expect(fixture.writes[0]!.body).toMatchObject({ input: text });
        expect(await saved()).toBeNull();
        expect(fixture.boundary.writes).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }
}
