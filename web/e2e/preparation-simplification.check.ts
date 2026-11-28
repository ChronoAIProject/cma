import { expect, test } from '@playwright/test';
import { acceptedAt, preparationFixture, progressAt } from './preparation-simplification.fixture';

const original = '  Original request 中文\n' + 'Keep this exact line and its spacing.\n'.repeat(8);
const followup = '  Separate follow-up 草稿\nNever send automatically  ';
for (const theme of ['dark', 'light']) {
  for (const width of [1440, 390, 320]) {
    test(`LOCAL preparation ${theme} ${width} keeps original bytes and reloadable unsent follow-up across binding`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const local = await preparationFixture(page, baseURL!);
      await page.goto('/agent-manager');
      await page.getByLabel('Your task', { exact: true }).fill(original);
      await page.getByRole('button', { name: 'Start task', exact: true }).click();
      const preparation = page.getByRole('region', { name: 'Task preparation', exact: true });
      await expect(
        preparation.getByRole('heading', { name: 'New agent', exact: true }),
      ).toBeVisible();
      await expect(preparation.locator(`time[datetime="${acceptedAt}"]`)).toBeVisible();
      await expect(preparation.locator(`time[datetime="${progressAt}"]`)).toBeVisible();
      await expect(preparation.locator('.agent-mascot__poster')).toHaveCount(1);
      await expect(preparation.locator('.agent-mascot__sprite')).toHaveCount(0);
      const message = preparation.getByRole('region', { name: 'Original request' });
      await message.getByRole('button', { name: 'Show more' }).click();
      expect(await message.locator('p').textContent()).toBe(original);
      const input = page.getByRole('textbox', { name: 'Follow-up message', exact: true });
      await expect(preparation.getByText('Follow-up draft', { exact: true })).toHaveCount(0);
      await input.fill(followup);
      await input.press('Control+Enter');
      await input.press('Meta+Enter');
      await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      const request = structuredClone(local.creates);
      expect(request).toHaveLength(1);
      expect(JSON.parse(request[0]!.body).initial_message).toBe(original);
      await page.clock.runFor(250);
      await page.screenshot({
        path: info.outputPath(`preparation-before-id-${theme}-${width}.png`),
        animations: 'disabled',
      });
      await page.reload();
      await expect(input).toHaveValue(followup);
      await expect(preparation.locator(`time[datetime="${acceptedAt}"]`)).toBeVisible();
      expect(local.creates).toEqual(request);
      local.bind();
      await page.clock.runFor(1500);
      await expect(page).toHaveURL(new RegExp(`/agents/${local.boundary.agent.id}$`));
      await expect(page.getByRole('region', { name: 'Task preparation', exact: true })).toHaveCount(
        1,
      );
      await expect(input).toHaveValue(followup);
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveCount(0);
      await expect(page.getByRole('region', { name: 'Agent provisioning progress' })).toBeVisible();
      await page.screenshot({
        path: info.outputPath(`preparation-bound-${theme}-${width}.png`),
        animations: 'disabled',
      });
      local.complete();
      await page.clock.runFor(3000);
      const messageDraft = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(messageDraft).toHaveValue(followup);
      await expect(preparation).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      expect(local.creates).toEqual(request);
      expect(local.boundary.writes).toEqual([]);
      expect(local.boundary.errors).toEqual([]);
      expect(local.boundary.unexpected).toEqual([]);
    });
  }
  for (const state of ['stalled', 'failed', 'read-error'] as const) {
    test(`LOCAL preparation ${theme} ${state} retains known times, original request and independent draft`, async ({
      page,
      baseURL,
    }, info) => {
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const local = await preparationFixture(page, baseURL!);
      await page.goto('/agent-manager');
      await page.getByLabel('Your task', { exact: true }).fill(original);
      await page.getByRole('button', { name: 'Start task', exact: true }).click();
      const preparation = page.getByRole('region', { name: 'Task preparation', exact: true });
      await expect(preparation.locator(`time[datetime="${acceptedAt}"]`)).toBeVisible();
      await page.getByRole('textbox', { name: 'Follow-up message', exact: true }).fill(followup);
      const request = structuredClone(local.creates);
      if (state === 'stalled') local.stall();
      else if (state === 'failed') local.fail();
      else local.readError(true);
      await page.clock.runFor(2000);
      const title =
        state === 'stalled'
          ? 'Setup is taking longer than expected'
          : state === 'failed'
            ? 'Setup needs attention'
            : 'Setup updates interrupted';
      await expect(preparation.getByRole('heading', { name: title, exact: true })).toBeVisible();
      await expect(preparation.locator(`time[datetime="${acceptedAt}"]`)).toBeVisible();
      await expect(preparation.locator(`time[datetime="${progressAt}"]`)).toBeVisible();
      await expect(preparation.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      await expect(
        page.getByRole('textbox', { name: 'Follow-up message', exact: true }),
      ).toHaveValue(followup);
      await expect(preparation.locator('.agent-mascot__poster')).toHaveCount(1);
      await expect(preparation.locator('.agent-mascot__sprite')).toHaveCount(0);
      if (state === 'failed')
        await expect(
          preparation
            .getByRole('alert')
            .filter({ hasText: 'Synthetic current setup failure' })
            .first(),
        ).toBeVisible();
      await page.screenshot({
        path: info.outputPath(`preparation-${state}-${theme}.png`),
        fullPage: true,
      });
      expect(local.creates).toEqual(request);
      expect(local.boundary.writes).toEqual([]);
      expect(local.boundary.errors).toEqual([]);
      expect(local.boundary.unexpected).toEqual([]);
    });
  }
}

test('LOCAL preparation plays one visible mascot and keeps its poster after it settles', async ({
  page,
  baseURL,
}) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  const local = await preparationFixture(page, baseURL!);
  await page.goto('/agent-manager');
  await page.getByLabel('Your task', { exact: true }).fill(original);
  await page.getByRole('button', { name: 'Start task', exact: true }).click();
  const preparation = page.getByRole('region', { name: 'Task preparation', exact: true });
  const current = preparation
    .getByRole('list', { name: 'Agent setup steps' })
    .locator('[aria-current="step"]');
  const marker = current.locator('.statedot__ping');
  await expect(marker).toHaveCSS('animation-name', 'statedot-ping');
  await expect(marker).toBeVisible();
  const sprite = preparation.locator('.agent-mascot__sprite > img:last-child');
  await expect(sprite).toBeVisible();
  await expect(page.locator('.agent-mascot__sprite')).toHaveCount(1);
  const firstFrame = await sprite.getAttribute('style');
  await page.clock.runFor(450);
  await expect(sprite).not.toHaveAttribute('style', firstFrame!);
  await page.clock.runFor(4000);
  await expect(sprite).toHaveCount(0);
  await expect(preparation.locator('.agent-mascot__poster')).toBeVisible();
  await page.clock.runFor(1500);
  await expect(sprite).toHaveCount(0);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await expect(marker).toHaveCSS('animation-name', 'none');
  await expect(marker).toBeHidden();
  await expect(current.getByText('In progress')).toBeVisible();
  expect(local.creates).toHaveLength(1);
  expect(local.boundary.writes).toEqual([]);
});
