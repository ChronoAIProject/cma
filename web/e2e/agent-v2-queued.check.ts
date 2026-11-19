import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: cancel a queued message and recover its exact lost receipt`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.queued.loseReceipt = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      { response_id: 'resp_active', command_id: 'cmd_active', turn_sequence: 1 },
    );
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(field).toBeVisible();
      await field.fill('A waiting message with a separate cancellation');
      await page.getByRole('button', { name: 'Queue message', exact: true }).click();
      const cancel = page.getByRole('button', { name: 'Remove queued message 1', exact: true });
      await expect(cancel).toBeEnabled();
      await field.fill('Keep this editable draft');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(cancel).toBeVisible();
        await expect
          .poll(() =>
            cancel.evaluate((element) => {
              const box = element.getBoundingClientRect();
              return (
                box.left >= 0 &&
                box.right <= innerWidth &&
                box.top >= 0 &&
                box.bottom <= innerHeight
              );
            }),
          )
          .toBe(true);
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({
          path: info.outputPath(`queued-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await cancel.focus();
      await page.keyboard.press('Enter');
      const recover = page.getByRole('button', { name: 'Recover request', exact: true });
      await expect(recover).toBeEnabled();
      await expect(field).toHaveValue('Keep this editable draft');
      await expect(page.getByRole('status').filter({ hasText: /^Thinking$/ })).toBeVisible();
      fixture.queued.conflict = true;
      await page.reload();
      await expect(recover).toBeEnabled();
      await recover.click();
      await expect(
        page.getByRole('alert').filter({ hasText: 'Cancellation observation changed' }),
      ).toBeVisible();
      await expect(recover).toBeEnabled();
      fixture.queued.conflict = false;
      await recover.click();
      await expect(
        page.getByRole('article', { name: 'Queued message 1', exact: true }),
      ).toHaveCount(0);
      await expect(recover).toHaveCount(0);
      await expect(page.getByRole('status').filter({ hasText: /^Thinking$/ })).toBeVisible();
      const writes = fixture.writes.filter((write) => write.path.endsWith('/queued-cancellation'));
      expect(writes).toHaveLength(3);
      expect(writes[1]).toEqual(writes[0]);
      expect(writes[2]).toEqual(writes[0]);
      expect(writes[0]!.body).toEqual({ source_command_id: 'cmd_1', expected_command_revision: 7 });
      expect(fixture.writes.filter((write) => write.path.endsWith('/controls'))).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
